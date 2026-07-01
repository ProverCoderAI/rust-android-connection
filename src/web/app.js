import {
  Adb,
  AdbAuthType,
  AdbCommand,
  AdbDaemonTransport,
  AdbPublicKeyAuthenticator,
  AdbSignatureAuthenticator,
  AdbSubprocessService,
} from "https://esm.sh/@yume-chan/adb@2.6.0?deps=@yume-chan/stream-extra@2.5.3";
import { AdbDaemonWebUsbDeviceManager } from "https://esm.sh/@yume-chan/adb-daemon-webusb@2.3.2?deps=@yume-chan/adb@2.6.0,@yume-chan/stream-extra@2.5.3";
import AdbWebCredentialStore from "https://esm.sh/@yume-chan/adb-credential-web@2.1.0?deps=@yume-chan/adb@2.6.0,@yume-chan/stream-extra@2.5.3";

const $ = (id) => document.getElementById(id);

const elements = {
  supportPill: $("support-pill"),
  contextPill: $("context-pill"),
  devicePill: $("device-pill"),
  refreshDevices: $("refresh-devices"),
  connectDevice: $("connect-device"),
  forgetDevices: $("forget-devices"),
  disconnectDevice: $("disconnect-device"),
  copyDiagnostics: $("copy-diagnostics"),
  connectionMessage: $("connection-message"),
  knownDevices: $("known-devices"),
  deviceSerial: $("device-serial"),
  deviceModel: $("device-model"),
  deviceAndroid: $("device-android"),
  shellForm: $("shell-form"),
  shellCommand: $("shell-command"),
  runShell: $("run-shell"),
  shellOutput: $("shell-output"),
  bridgeActivity: $("bridge-activity"),
  captureScreen: $("capture-screen"),
  screenImage: $("screen-image"),
  screenFrame: $("screen-image").closest(".screenshot-frame"),
  downloadScreen: $("download-screen"),
  sessionLog: $("session-log"),
};

const state = {
  manager: null,
  backend: null,
  connection: null,
  adb: null,
  subprocess: null,
  screenUrl: null,
  connecting: false,
  authPublicKeySent: false,
  authPublicKeyResolve: null,
  diagnostics: [],
  instrumentedUsbDevices: new WeakSet(),
  bridgeClientId: null,
  bridgePollingEnabled: true,
  bridgeBusy: false,
  bridgeActivityLimit: 8,
  installCounter: 0,
};

const runtimeFlags = {
  usbDebug: new URLSearchParams(window.location.search).get("usb_debug") === "1",
};

const packageVersions = {
  adb: "@yume-chan/adb@2.6.0 with @yume-chan/stream-extra@2.5.3",
  webusb: "@yume-chan/adb-daemon-webusb@2.3.2 with shared @yume-chan/stream-extra@2.5.3",
  credential: "@yume-chan/adb-credential-web@2.1.0 with shared @yume-chan/stream-extra@2.5.3",
};

const setPill = (element, text, tone) => {
  element.textContent = text;
  element.className = `status-pill ${tone}`;
};

const log = (message) => {
  const now = new Date();
  const entry = {
    time: now.toISOString(),
    message,
  };
  state.diagnostics.push(entry);
  postClientLog(entry);
  const time = now.toLocaleTimeString();
  elements.sessionLog.textContent = `${elements.sessionLog.textContent}[${time}] ${message}\n`;
  elements.sessionLog.scrollTop = elements.sessionLog.scrollHeight;
};

const postClientLog = (entry) => {
  const payload = JSON.stringify({
    ...entry,
    location: window.location.href,
    clientId: state.bridgeClientId || "-",
    serial: elements.deviceSerial?.textContent || "-",
    state: elements.devicePill?.textContent || "-",
  });
  try {
    if (navigator.sendBeacon) {
      navigator.sendBeacon("/client-log", new Blob([payload], { type: "application/json" }));
      return;
    }
    fetch("/client-log", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: payload,
      keepalive: true,
    }).catch(() => {});
  } catch {
    /* diagnostics must not affect WebUSB control flow */
  }
};

const bridgeHeaders = (headers = {}) =>
  state.bridgeClientId
    ? { ...headers, "X-Bridge-Client-Id": state.bridgeClientId }
    : headers;

const registerBridgeClient = async () => {
  const response = await fetch("/bridge/client/session", {
    method: "POST",
    cache: "no-store",
  });
  if (!response.ok) {
    throw new Error(`bridge client session failed: HTTP ${response.status}`);
  }
  const payload = await response.json();
  state.bridgeClientId = payload.clientId || null;
  state.bridgePollingEnabled = Boolean(state.bridgeClientId);
  log(`Bridge client session: ${state.bridgeClientId || "unavailable"}`);
};

const setMessage = (message, tone = "") => {
  elements.connectionMessage.textContent = message;
  elements.connectionMessage.className = `connection-message ${tone}`;
};

const renderDisconnected = () => {
  state.backend = null;
  state.connection = null;
  state.adb = null;
  state.subprocess = null;
  state.connecting = false;
  setPill(elements.devicePill, "No device", "warn");
  elements.disconnectDevice.disabled = true;
  elements.runShell.disabled = true;
  elements.captureScreen.disabled = true;
  elements.deviceSerial.textContent = "-";
  elements.deviceModel.textContent = "-";
  elements.deviceAndroid.textContent = "-";
};

const renderConnected = (serial) => {
  setPill(elements.devicePill, serial || "Connected", "ok");
  elements.disconnectDevice.disabled = false;
  elements.runShell.disabled = false;
  elements.captureScreen.disabled = false;
  elements.deviceSerial.textContent = serial || "-";
};

const renderAuthorizing = (serial) => {
  setPill(elements.devicePill, "Authorizing", "warn");
  elements.disconnectDevice.disabled = true;
  elements.runShell.disabled = true;
  elements.captureScreen.disabled = true;
  elements.deviceSerial.textContent = serial || "-";
};

const recordDiagnosticError = (label, error) => {
  log(`${label}: ${formatError(error)}`);
  if (error instanceof Error && error.stack) {
    state.diagnostics.push({
      time: new Date().toISOString(),
      message: `${label} stack`,
      stack: error.stack,
    });
  }
};

const assertReady = () => {
  if (!state.adb) {
    throw new Error("Connect a phone first");
  }
  return state.adb;
};

const instrumentAuthenticator = (label, authenticator, onResponse) =>
  async function* authenticatedWithDiagnostics(credentialStore, getNextRequest) {
    let requestLogged = false;
    const loggedGetNextRequest = async () => {
      const packet = await getNextRequest();
      if (!requestLogged) {
        requestLogged = true;
        log(
          `${label}: auth request type=${packet.arg0}, payload=${packet.payload?.byteLength || packet.payload?.length || 0} bytes`,
        );
      }
      return packet;
    };

    for await (const packet of authenticator(credentialStore, loggedGetNextRequest)) {
      onResponse(packet);
      yield packet;
    }
  };

const instrumentedSignatureAuthenticator = instrumentAuthenticator(
  "Signature auth",
  AdbSignatureAuthenticator,
  (packet) => {
    if (packet.command === AdbCommand.Auth && packet.arg0 === AdbAuthType.Signature) {
      log(`ADB signature response sent to Android`);
    }
  },
);

const instrumentedPublicKeyAuthenticator = instrumentAuthenticator(
  "Public key auth",
  AdbPublicKeyAuthenticator,
  (packet) => {
    if (packet.command === AdbCommand.Auth && packet.arg0 === AdbAuthType.PublicKey) {
      state.authPublicKeySent = true;
      state.authPublicKeyResolve?.();
      state.authPublicKeyResolve = null;
      log(`ADB public key sent to Android: ${packet.payload?.byteLength || packet.payload?.length || 0} bytes`);
      setMessage(
        "ADB public key was sent. Unlock the phone and allow USB debugging.",
        "warn",
      );
    }
  },
);

const closeConnection = async (connection) => {
  if (!connection) {
    return;
  }
  try {
    await connection.writable?.abort?.("closing stale WebUSB ADB connection");
  } catch (error) {
    log(`WebUSB writable cleanup warning: ${formatError(error)}`);
  }
  try {
    await connection.readable?.cancel?.("closing stale WebUSB ADB connection");
  } catch (error) {
    log(`WebUSB readable cleanup warning: ${formatError(error)}`);
  }
  try {
    await connection.device?.raw?.close?.();
  } catch (error) {
    log(`USB device cleanup warning: ${formatError(error)}`);
  }
};

const closeBackendRaw = async (backend) => {
  try {
    await backend?.raw?.close?.();
  } catch (error) {
    log(`USB raw close warning: ${formatError(error)}`);
  }
};

const logBackendDetails = (backend) => {
  const raw = backend.raw;
  if (!raw) {
    log("USB device details unavailable");
    return;
  }
  const vendorId = raw.vendorId?.toString(16).padStart(4, "0") || "unknown";
  const productId = raw.productId?.toString(16).padStart(4, "0") || "unknown";
  log(
    `USB device selected: vendor=0x${vendorId}, product=0x${productId}, name=${raw.productName || "-"}, serial=${raw.serialNumber || "-"}`,
  );
};

const delay = (milliseconds) =>
  new Promise((resolve) => {
    window.setTimeout(resolve, milliseconds);
  });

const resetWebUsbConnection = async (connection) => {
  const raw = connection?.device?.raw;
  const endpoint = connection?.outEndpoint;
  if (!raw || !endpoint) {
    log("ADB reset skipped: WebUSB endpoint details unavailable");
    return;
  }

  const resetPacket = new Uint8Array(endpoint.packetSize || 64);
  for (let attempt = 0; attempt < 10; attempt += 1) {
    await raw.transferOut(endpoint.endpointNumber, resetPacket);
  }
  log(`ADB reset sent on USB OUT endpoint ${endpoint.endpointNumber}`);
};

const asUint8Array = (data) => {
  if (data instanceof Uint8Array) {
    return data;
  }
  if (ArrayBuffer.isView(data)) {
    return new Uint8Array(data.buffer, data.byteOffset, data.byteLength);
  }
  if (data instanceof ArrayBuffer) {
    return new Uint8Array(data);
  }
  return new Uint8Array();
};

const consumeMaybe = async (value, consumer) => {
  if (value && typeof value.tryConsume === "function") {
    await value.tryConsume(consumer);
    return;
  }
  await consumer(value?.value ?? value);
};

const calculateChecksum = (payload) => payload.reduce((sum, byte) => (sum + byte) >>> 0, 0);

const readUint32LittleEndian = (bytes, offset) =>
  (
    bytes[offset] |
    (bytes[offset + 1] << 8) |
    (bytes[offset + 2] << 16) |
    (bytes[offset + 3] << 24)
  ) >>> 0;

const hexPreview = (bytes, limit = 48) =>
  Array.from(bytes.slice(0, limit), (byte) => byte.toString(16).padStart(2, "0")).join(" ");

const asciiPreview = (bytes, limit = 32) =>
  Array.from(bytes.slice(0, limit), (byte) =>
    byte >= 0x20 && byte <= 0x7e ? String.fromCharCode(byte) : ".",
  ).join("");

const adbFramePreview = (bytes) => {
  if (bytes.byteLength < 24) {
    return "";
  }
  const command = readUint32LittleEndian(bytes, 0);
  const magic = readUint32LittleEndian(bytes, 20);
  if (((command ^ 0xffffffff) >>> 0) !== magic) {
    return "";
  }

  const name = asciiPreview(bytes.slice(0, 4), 4);
  const arg0 = readUint32LittleEndian(bytes, 4);
  const arg1 = readUint32LittleEndian(bytes, 8);
  const payloadLength = readUint32LittleEndian(bytes, 12);
  return ` adb=${name} arg0=${arg0} arg1=${arg1} payload=${payloadLength}`;
};

const usbBytesPreview = (data) => {
  const bytes = asUint8Array(data);
  if (bytes.byteLength === 0) {
    return "0 bytes";
  }
  return `${bytes.byteLength} bytes${adbFramePreview(bytes)} hex=[${hexPreview(bytes)}] ascii="${asciiPreview(bytes)}"`;
};

const instrumentRawUsbDevice = (raw, label) => {
  if (!raw || state.instrumentedUsbDevices.has(raw)) {
    return;
  }

  const originalTransferOut = raw.transferOut.bind(raw);
  const originalTransferIn = raw.transferIn.bind(raw);
  raw.transferOut = async (endpointNumber, data) => {
    const startedAt = performance.now();
    log(`USB OUT ${label} ep=${endpointNumber}: ${usbBytesPreview(data)}`);
    try {
      const result = await originalTransferOut(endpointNumber, data);
      log(
        `USB OUT ${label} ep=${endpointNumber} complete: status=${result.status || "ok"} bytesWritten=${result.bytesWritten ?? "?"} elapsed=${Math.round(performance.now() - startedAt)}ms`,
      );
      return result;
    } catch (error) {
      log(
        `USB OUT ${label} ep=${endpointNumber} failed after ${Math.round(performance.now() - startedAt)}ms: ${formatError(error)}`,
      );
      throw error;
    }
  };
  raw.transferIn = async (endpointNumber, length) => {
    const startedAt = performance.now();
    log(`USB IN ${label} ep=${endpointNumber}: requested ${length} bytes`);
    try {
      const result = await originalTransferIn(endpointNumber, length);
      log(
        `USB IN ${label} ep=${endpointNumber} complete: status=${result.status || "ok"} ${usbBytesPreview(result.data)} elapsed=${Math.round(performance.now() - startedAt)}ms`,
      );
      return result;
    } catch (error) {
      log(
        `USB IN ${label} ep=${endpointNumber} failed after ${Math.round(performance.now() - startedAt)}ms: ${formatError(error)}`,
      );
      throw error;
    }
  };
  state.instrumentedUsbDevices.add(raw);
  log(`USB raw diagnostics enabled for ${label}`);
};

const writeUint32LittleEndian = (view, offset, value) => {
  view.setUint32(offset, value >>> 0, true);
};

const serializeAdbPacket = (packet) => {
  const payload = asUint8Array(packet?.payload);
  const command = packet.command >>> 0;
  const checksum = packet.checksum ?? calculateChecksum(payload);
  const magic = packet.magic ?? ((command ^ 0xffffffff) >>> 0);
  const header = new Uint8Array(24);
  const view = new DataView(header.buffer);
  writeUint32LittleEndian(view, 0, command);
  writeUint32LittleEndian(view, 4, packet.arg0 ?? 0);
  writeUint32LittleEndian(view, 8, packet.arg1 ?? 0);
  writeUint32LittleEndian(view, 12, payload.byteLength);
  writeUint32LittleEndian(view, 16, checksum);
  writeUint32LittleEndian(view, 20, magic);
  return { header, payload };
};

const writeUsbChunk = async (raw, endpoint, chunk) => {
  await raw.transferOut(endpoint.endpointNumber, chunk);
  const packetSize = endpoint.packetSize || 64;
  if (chunk.byteLength > 0 && chunk.byteLength % packetSize === 0) {
    await raw.transferOut(endpoint.endpointNumber, new Uint8Array());
  }
};

const createConnectionWithLocalAdbSerializer = (connection) => {
  const raw = connection.device?.raw;
  const endpoint = connection.outEndpoint;
  if (!raw || !endpoint) {
    log("Local ADB serializer skipped: WebUSB endpoint details unavailable");
    return connection;
  }

  const writable = new WritableStream({
    async write(packetOrConsumable) {
      await consumeMaybe(packetOrConsumable, async (packet) => {
        const { header, payload } = serializeAdbPacket(packet);
        await writeUsbChunk(raw, endpoint, header);
        if (payload.byteLength > 0) {
          await writeUsbChunk(raw, endpoint, payload);
        }
      });
    },
    async close() {
      await raw.close();
    },
    async abort() {
      await raw.close();
    },
  });

  log("Local ADB packet serializer enabled for WebUSB writes");
  return {
    device: connection.device,
    readable: connection.readable,
    writable,
    inEndpoint: connection.inEndpoint,
    outEndpoint: connection.outEndpoint,
  };
};

const renderKnownDevices = (devices) => {
  elements.knownDevices.replaceChildren();
  if (devices.length === 0) {
    const item = document.createElement("li");
    item.textContent = "No authorized WebUSB devices";
    elements.knownDevices.append(item);
    return;
  }
  for (const device of devices) {
    const item = document.createElement("li");
    item.className = "known-device";
    const name = device.name || device.serial || "Android USB device";
    const label = document.createElement("span");
    label.textContent = device.serial ? `${name} (${device.serial})` : name;
    const button = document.createElement("button");
    button.type = "button";
    button.textContent = "Connect";
    button.addEventListener("click", () => {
      button.disabled = true;
      connectBackend(device)
        .catch((error) => {
          renderDisconnected();
          setMessage(`Connect failed: ${formatError(error)}`, "bad");
          recordDiagnosticError("Connect failed", error);
        })
        .finally(() => {
          button.disabled = false;
          elements.connectDevice.disabled = false;
        });
    });
    item.append(label, button);
    elements.knownDevices.append(item);
  }
};

const refreshKnownDevices = async () => {
  if (!state.manager) {
    return;
  }
  try {
    const devices = await state.manager.getDevices();
    renderKnownDevices(devices);
    log(`Known devices: ${devices.length}`);
  } catch (error) {
    log(`Device refresh failed: ${formatError(error)}`);
  }
};

const forgetKnownDevices = async () => {
  if (!state.manager) {
    return;
  }
  try {
    await disconnectDevice();
    const devices = await state.manager.getDevices();
    for (const device of devices) {
      await device.raw?.forget?.();
    }
    await clearCredentialStore();
    renderDisconnected();
    renderKnownDevices([]);
    setMessage("USB browser permission and ADB browser keys were forgotten. Replug the phone, then click Connect phone.", "ok");
    log(`Forgot USB permission for ${devices.length} device(s) and cleared ADB browser keys`);
  } catch (error) {
    setMessage(`Forget USB permission failed: ${formatError(error)}`, "bad");
    recordDiagnosticError("Forget USB permission failed", error);
  }
};

const clearCredentialStore = () =>
  new Promise((resolve, reject) => {
    const request = indexedDB.deleteDatabase("Tango");
    request.onerror = () => reject(request.error || new Error("IndexedDB delete failed"));
    request.onblocked = () => {
      log("ADB browser key reset is blocked by another open tab for this origin");
      resolve();
    };
    request.onsuccess = () => resolve();
  });

const connectDevice = async () => {
  try {
    if (!state.manager) {
      throw new Error("WebUSB is unavailable in this browser");
    }
    if (state.connecting) {
      throw new Error("A connection attempt is already in progress");
    }
    elements.connectDevice.disabled = true;
    if (!state.bridgePollingEnabled || !state.bridgeClientId) {
      await registerBridgeClient();
    }
    setMessage("Waiting for browser USB permission", "warn");
    log("Requesting USB device access");
    const backend = await state.manager.requestDevice();
    if (!backend) {
      throw new Error("No USB device selected");
    }
    await connectBackend(backend);
  } catch (error) {
    renderDisconnected();
    setMessage(`Connect failed: ${formatError(error)}`, "bad");
    recordDiagnosticError("Connect failed", error);
  } finally {
    elements.connectDevice.disabled = false;
  }
};

const connectBackend = async (backend) => {
  if (state.connecting && state.backend !== backend) {
    throw new Error("A connection attempt is already in progress");
  }
  state.connecting = true;
  let activeBackend = backend;
  const serial = backend.serial || backend.raw?.serialNumber || "Android device";
  elements.connectDevice.disabled = true;
  renderAuthorizing(serial);
  setMessage("Opening USB ADB interface", "warn");
  log(`Opening ADB transport for ${serial}`);
  logBackendDetails(activeBackend);
  log("Step: backend.connect()");
  try {
    const authResult = await authenticateBackendWithRetries(activeBackend, serial);
    activeBackend = authResult.backend;
    const transport = authResult.transport;
    log("Step: AdbDaemonTransport.authenticate() complete");
    state.backend = activeBackend;
    state.connection = null;
    state.adb = new Adb(transport);
    state.subprocess = new AdbSubprocessService(state.adb);
    renderConnected(serial);
    await hydrateDeviceFacts();
    await refreshKnownDevices();
    setMessage("ADB connected", "ok");
    log("ADB connected. Accept the RSA prompt on the phone if Android asks.");
  } catch (error) {
    await closeBackendRaw(activeBackend);
    throw error;
  } finally {
    state.connecting = false;
    elements.connectDevice.disabled = false;
  }
};

const authenticateBackendWithRetries = async (initialBackend, serial) => {
  let currentBackend = initialBackend;
  let lastError = null;

  for (let attempt = 1; attempt <= 3; attempt += 1) {
    let rawConnection = null;
    let connection = null;
    try {
      log(`ADB handshake attempt ${attempt}`);
      const openResult = await openBackendConnection(currentBackend, serial);
      currentBackend = openResult.backend;
      rawConnection = openResult.connection;
      log("Step: backend.connect() complete");
      if (runtimeFlags.usbDebug) {
        instrumentRawUsbDevice(rawConnection.device?.raw, serial);
      }
      connection = createConnectionWithLocalAdbSerializer(rawConnection);
      state.connection = connection;
      state.authPublicKeySent = false;

      const credentialStore = new AdbWebCredentialStore("rust-android-connection");
      log("Step: AdbDaemonTransport.authenticate()");
      setMessage(
        "Waiting for Android RSA authorization. Unlock the phone and tap Allow USB debugging.",
        "warn",
      );

      const publicKeyPromise = new Promise((resolve) => {
        state.authPublicKeyResolve = resolve;
      });
      const authPromise = AdbDaemonTransport.authenticate({
        serial,
        connection,
        credentialStore,
        authenticators: [
          instrumentedSignatureAuthenticator,
          instrumentedPublicKeyAuthenticator,
        ],
        initialDelayedAckBytes: 0,
        readTimeLimit: 1000,
      });
      authPromise.catch(() => {});

      const earlyResult = await Promise.race([
        authPromise.then((transport) => ({ type: "transport", transport })),
        publicKeyPromise.then(() => ({ type: "public-key" })),
        delay(9000).then(() => {
          if (!state.authPublicKeySent) {
            throw new Error("ADB handshake produced no response before Android authorization");
          }
          return { type: "public-key" };
        }),
      ]);

      if (earlyResult.type === "transport") {
        state.connection = null;
        state.authPublicKeyResolve = null;
        return {
          backend: currentBackend,
          transport: earlyResult.transport,
        };
      }

      const transport = await withTimeout(
        authPromise,
        90000,
        "ADB authorization timed out. Unlock the phone, check for the Allow USB debugging prompt, or revoke USB debugging authorizations and reconnect.",
        {
          onTimeout: () => closeBackendRaw(currentBackend),
          onLateValue: (transportAfterTimeout) => transportAfterTimeout?.close?.(),
        },
      );
      state.connection = null;
      state.authPublicKeyResolve = null;
      return {
        backend: currentBackend,
        transport,
      };
    } catch (error) {
      lastError = error;
      recordDiagnosticError(`ADB handshake attempt ${attempt} failed`, error);
      state.authPublicKeyResolve = null;
      if (rawConnection && !state.authPublicKeySent) {
        await resetWebUsbConnection(rawConnection).catch((resetError) => {
          recordDiagnosticError("ADB handshake reset failed", resetError);
        });
      }
      await closeConnection(connection);
      await closeBackendRaw(currentBackend);
      state.connection = null;

      if (state.authPublicKeySent) {
        break;
      }

      await delay(900);
      currentBackend = await findFreshBackendBySerial(serial, currentBackend).catch((refreshError) => {
        recordDiagnosticError("WebUSB re-enumeration failed", refreshError);
        return currentBackend;
      });
    }
  }

  throw lastError || new Error("ADB handshake failed");
};

const openBackendConnection = async (initialBackend, serial) => {
  let lastError = null;
  let currentBackend = initialBackend;

  for (let attempt = 1; attempt <= 3; attempt += 1) {
    try {
      if (attempt > 1) {
        log(`Retrying WebUSB open for ${serial}: attempt ${attempt}`);
        await delay(800);
        currentBackend = await findFreshBackendBySerial(serial, currentBackend);
        logBackendDetails(currentBackend);
      }

      const connection = await withTimeout(
        currentBackend.connect(),
        15000,
        "Opening WebUSB ADB interface timed out. Close adb.exe, Android Studio, Phone Link, and other phone tools, then replug USB.",
        {
          onTimeout: () => closeBackendRaw(currentBackend),
          onLateValue: closeConnection,
        },
      );
      return {
        backend: currentBackend,
        connection,
      };
    } catch (error) {
      lastError = error;
      recordDiagnosticError(`backend.connect() attempt ${attempt} failed`, error);
      await closeBackendRaw(currentBackend);
      if (!isDisconnectedUsbError(error)) {
        break;
      }
    }
  }

  throw lastError || new Error("Opening WebUSB ADB interface failed");
};

const findFreshBackendBySerial = async (serial, fallbackBackend) => {
  if (!state.manager || serial === "Android device") {
    return fallbackBackend;
  }

  const devices = await state.manager.getDevices({
    filters: [{ serialNumber: serial }],
  });
  const [freshBackend] = devices;
  if (!freshBackend) {
    throw new Error(`USB device ${serial} is not available after re-enumeration`);
  }
  return freshBackend;
};

const isDisconnectedUsbError = (error) =>
  formatError(error).toLowerCase().includes("device was disconnected");

const withTimeout = (operation, timeoutMs, message, options = {}) =>
  new Promise((resolve, reject) => {
    let settled = false;
    const timeout = window.setTimeout(() => {
      settled = true;
      Promise.resolve(options.onTimeout?.())
        .catch((error) => {
          recordDiagnosticError("Timeout cleanup failed", error);
        })
        .finally(() => reject(new Error(message)));
    }, timeoutMs);
    operation.then(
      (value) => {
        if (settled) {
          Promise.resolve(options.onLateValue?.(value)).catch((error) => {
            recordDiagnosticError("Late timeout value cleanup failed", error);
          });
          return;
        }
        settled = true;
        window.clearTimeout(timeout);
        resolve(value);
      },
      (error) => {
        if (settled) {
          return;
        }
        settled = true;
        window.clearTimeout(timeout);
        reject(error);
      },
    );
  });

const disconnectDevice = async () => {
  try {
    await state.adb?.close?.();
    await closeConnection(state.connection);
  } catch (error) {
    recordDiagnosticError("Disconnect warning", error);
  } finally {
    renderDisconnected();
    log("Disconnected");
  }
};

const hydrateDeviceFacts = async () => {
  try {
    const model = await state.adb.getProp("ro.product.model");
    const android = await state.adb.getProp("ro.build.version.release");
    elements.deviceModel.textContent = model.trim() || "-";
    elements.deviceAndroid.textContent = android.trim() || "-";
  } catch (error) {
    log(`Device facts unavailable: ${formatError(error)}`);
  }
};

const shellText = async (command) => {
  assertReady();
  if (state.subprocess?.noneProtocol?.spawnWaitText) {
    return state.subprocess.noneProtocol.spawnWaitText(command);
  }
  if (state.subprocess?.shellProtocol?.isSupported && state.subprocess.shellProtocol.spawnWaitText) {
    return state.subprocess.shellProtocol.spawnWaitText(command);
  }
  throw new Error("Current WebADB package does not expose shell subprocess APIs");
};

const shellBytes = async (command) => {
  assertReady();
  if (!state.subprocess?.noneProtocol?.spawnWait) {
    throw new Error("Current WebADB package does not expose binary subprocess output");
  }
  return state.subprocess.noneProtocol.spawnWait(command);
};

const bytesToBase64 = (bytes) => {
  let binary = "";
  const chunkSize = 0x8000;
  for (let offset = 0; offset < bytes.byteLength; offset += chunkSize) {
    const chunk = bytes.slice(offset, offset + chunkSize);
    binary += String.fromCharCode(...chunk);
  }
  return btoa(binary);
};

const displayScreenshotBytes = (bytes, label) => {
  const blob = new Blob([bytes], { type: "image/png" });
  if (state.screenUrl) {
    URL.revokeObjectURL(state.screenUrl);
  }
  state.screenUrl = URL.createObjectURL(blob);
  elements.screenImage.src = state.screenUrl;
  elements.downloadScreen.href = state.screenUrl;
  elements.downloadScreen.hidden = false;
  elements.screenFrame.classList.add("has-image");
  log(`${label}: ${blob.size} bytes`);
  return { size: blob.size, url: state.screenUrl };
};

const adbShellCommandLine = (parts) => {
  if (parts.length === 0) {
    return "";
  }
  if (parts.length === 1) {
    return parts[0];
  }
  return parts.map((part) => {
    if (/^[A-Za-z0-9_./:=,@%+-]+$/.test(part)) {
      return part;
    }
    return `'${part.replaceAll("'", "'\\''")}'`;
  }).join(" ");
};

const normalizedBridgeAdbArgs = (args) => {
  const normalized = [...args];
  if (normalized[0] === "-s" && normalized.length >= 2) {
    normalized.splice(0, 2);
  }
  if (normalized[0] === "-d" || normalized[0] === "-e") {
    normalized.shift();
  }
  if (normalized[0] === "wait-for-device") {
    normalized.shift();
  }
  return normalized;
};

const isBridgeScreenshotCommand = (normalized) =>
  normalized[0] === "exec-out" && normalized[1] === "screencap" && normalized.includes("-p");

const isBridgeInstallCommand = (normalized) => normalized[0] === "install";

const bridgeCommandLabel = (command) => {
  const args = Array.isArray(command.args) ? command.args : [];
  return [command.kind, ...args].join(" ").trim();
};

const previewText = (text, limit = 900) => {
  const value = String(text).trim();
  if (value.length === 0) {
    return "";
  }
  return value.length > limit ? `${value.slice(0, limit)}\n...` : value;
};

const base64ByteLength = (value) => {
  const padding = value.endsWith("==") ? 2 : value.endsWith("=") ? 1 : 0;
  return Math.max(0, Math.floor((value.length * 3) / 4) - padding);
};

const removeBridgeActivityItem = (item) => {
  item.querySelectorAll("img[data-object-url]").forEach((image) => {
    URL.revokeObjectURL(image.dataset.objectUrl);
  });
  item.remove();
};

const addBridgeActivity = (command) => {
  if (!elements.bridgeActivity) {
    return null;
  }

  const empty = elements.bridgeActivity.querySelector(".activity-empty");
  empty?.remove();

  const item = document.createElement("li");
  item.className = "activity-item warn";

  const step = document.createElement("div");
  step.className = "activity-step";
  step.textContent = command.id || "?";

  const flow = document.createElement("div");
  flow.className = "activity-flow";

  const callCard = document.createElement("section");
  callCard.className = "activity-card";
  const callHeader = document.createElement("div");
  callHeader.className = "activity-card-header";
  const callTitle = document.createElement("span");
  callTitle.className = "activity-card-title";
  callTitle.textContent = "Tool call";
  callHeader.append(callTitle);
  const callCode = document.createElement("pre");
  callCode.className = "activity-code";
  callCode.textContent = `rust-android-connection phone ${bridgeCommandLabel(command)}`;
  callCard.append(callHeader, callCode);

  const arrow = document.createElement("div");
  arrow.className = "activity-arrow";
  arrow.textContent = "->";

  const resultCard = document.createElement("section");
  resultCard.className = "activity-card activity-result warn";
  const resultHeader = document.createElement("div");
  resultHeader.className = "activity-card-header";
  const resultTitle = document.createElement("span");
  resultTitle.className = "activity-card-title";
  resultTitle.textContent = "Result";
  const status = document.createElement("span");
  status.className = "activity-status warn";
  status.textContent = "running";
  resultHeader.append(resultTitle, status);

  const detail = document.createElement("pre");
  detail.className = "activity-detail";
  detail.textContent = `Started at ${new Date().toLocaleTimeString()}`;
  resultCard.append(resultHeader, detail);

  flow.append(callCard, arrow, resultCard);
  item.append(step, flow);
  elements.bridgeActivity.prepend(item);
  while (elements.bridgeActivity.children.length > state.bridgeActivityLimit) {
    removeBridgeActivityItem(elements.bridgeActivity.lastElementChild);
  }

  return { item, status, resultCard, detail };
};

const updateBridgeActivity = (activity, tone, status, detail, ui = {}) => {
  if (!activity) {
    return;
  }
  activity.item.className = `activity-item ${tone}`;
  activity.resultCard.className = `activity-card activity-result ${tone}`;
  activity.status.className = `activity-status ${tone}`;
  activity.status.textContent = status;
  activity.detail.textContent = detail || "Completed with no output";
  const previousImage = activity.resultCard.querySelector(".activity-screenshot");
  if (previousImage?.dataset.objectUrl) {
    URL.revokeObjectURL(previousImage.dataset.objectUrl);
  }
  previousImage?.remove();
  if (ui.screenshot?.url) {
    const image = document.createElement("img");
    image.className = "activity-screenshot";
    image.src = ui.screenshot.url;
    image.dataset.objectUrl = ui.screenshot.url;
    image.alt = "Android screenshot result";
    activity.resultCard.append(image);
  }
};

const bridgeResultDetail = (command, result) => {
  if (result.error) {
    return result.error;
  }
  if (result.stderr) {
    return previewText(result.stderr);
  }
  if (result.stdout) {
    return previewText(result.stdout) || "Completed with empty output";
  }
  if (result.ui?.screenshot) {
    return `PNG displayed in Screenshot panel (${result.ui.screenshot.size} bytes)`;
  }
  if (result.stdoutBase64) {
    const normalized = command.kind === "adb" ? normalizedBridgeAdbArgs(command.args || []) : [];
    const bytes = base64ByteLength(result.stdoutBase64);
    if (isBridgeScreenshotCommand(normalized)) {
      return `Screenshot displayed in Screenshot panel (${bytes} bytes)`;
    }
    return `Binary output returned to CLI (${bytes} bytes)`;
  }
  return "Completed with no output";
};

const safeRemoteApkName = (name) => {
  const sanitized = String(name || "app.apk").replace(/[^A-Za-z0-9._-]/g, "_");
  return sanitized.endsWith(".apk") ? sanitized : `${sanitized}.apk`;
};

const installRemotePath = (file) => {
  state.installCounter += 1;
  return `/data/local/tmp/rust-android-connection-${Date.now()}-${state.installCounter}-${safeRemoteApkName(file?.name)}`;
};

const adbInstallFlags = (normalized) => {
  if (normalized.length < 2) {
    throw new Error("adb install requires an APK path");
  }
  return normalized.slice(1, -1);
};

const streamBytesToWritable = async (bytes, writable, label) => {
  const writer = writable.getWriter();
  const chunkSize = 64 * 1024;
  let nextProgress = 16 * 1024 * 1024;
  try {
    for (let offset = 0; offset < bytes.byteLength; offset += chunkSize) {
      const end = Math.min(offset + chunkSize, bytes.byteLength);
      await writer.write(bytes.subarray(offset, end));
      if (end >= nextProgress || end === bytes.byteLength) {
        log(`${label}: streamed ${end}/${bytes.byteLength} bytes`);
        nextProgress += 16 * 1024 * 1024;
      }
    }
    await writer.close();
  } finally {
    writer.releaseLock();
  }
};

const installBridgeFileViaPmStdin = async (bytes, flags) => {
  if (!state.subprocess?.shellProtocol?.isSupported || !state.subprocess.shellProtocol.spawn) {
    return null;
  }
  const command = ["pm", "install", "-S", String(bytes.byteLength), ...flags];
  log(`Install: ${adbShellCommandLine(command)} < APK`);
  const process = await state.subprocess.shellProtocol.spawn(command);
  const stdoutChunks = [];
  const stdoutPromise = process.stdout.pipeThrough(new TextDecoderStream()).pipeTo(new WritableStream({
    write(chunk) {
      stdoutChunks.push(chunk);
      log(`Install stdout: ${previewText(chunk, 300)}`);
    },
  }));
  const stderrChunks = [];
  const stderrPromise = process.stderr.pipeThrough(new TextDecoderStream()).pipeTo(new WritableStream({
    write(chunk) {
      stderrChunks.push(chunk);
      log(`Install stderr: ${previewText(chunk, 300)}`);
    },
  }));
  await streamBytesToWritable(bytes, process.stdin, "Install stdin");
  const exitCode = await process.exited;
  await Promise.allSettled([stdoutPromise, stderrPromise]);
  return {
    exitCode,
    stdout: stdoutChunks.join(""),
    stderr: stderrChunks.join(""),
  };
};

const pushBridgeFile = async (file, remotePath) => {
  if (!file?.url) {
    throw new Error("Bridge install command did not include an APK attachment");
  }
  const response = await fetch(file.url, { cache: "no-store" });
  if (!response.ok) {
    throw new Error(`APK download failed: HTTP ${response.status}`);
  }
  const bytes = new Uint8Array(await response.arrayBuffer());
  log(`Install: downloaded APK attachment (${bytes.byteLength} bytes)`);
  if (file.size && bytes.byteLength !== file.size) {
    throw new Error(`APK download size mismatch: expected ${file.size}, got ${bytes.byteLength}`);
  }
  return bytes;
};

const pushBridgeFileViaSync = async (bytes, remotePath) => {
  const chunkSize = 64 * 1024;
  let offset = 0;
  let nextProgress = 16 * 1024 * 1024;
  const apkStream = new ReadableStream({
    pull(controller) {
      if (offset >= bytes.byteLength) {
        controller.close();
        return;
      }
      const end = Math.min(offset + chunkSize, bytes.byteLength);
      controller.enqueue(bytes.subarray(offset, end));
      offset = end;
      if (offset >= nextProgress || offset === bytes.byteLength) {
        log(`Install sync: streamed ${offset}/${bytes.byteLength} bytes to ADB sync`);
        nextProgress += 16 * 1024 * 1024;
      }
    },
  });
  const sync = await state.adb.sync();
  try {
    log(`Install: pushing APK to Android sync (${bytes.byteLength} bytes)`);
    await sync.write({
      filename: remotePath,
      file: apkStream,
      permission: 0o644,
    });
  } finally {
    await sync.dispose();
  }
  log("Install: APK push complete");
};

const executeBridgeInstallCommand = async (command, normalized) => {
  assertReady();
  const remotePath = installRemotePath(command.file);
  const size = command.file?.size ? `${Math.round(command.file.size / 1024 / 1024)} MiB` : "unknown size";
  log(`Install: pushing ${command.file?.name || "APK"} (${size}) to ${remotePath}`);
  const flags = adbInstallFlags(normalized);
  const bytes = await pushBridgeFile(command.file, remotePath);
  const stdinInstall = await installBridgeFileViaPmStdin(bytes, flags);
  if (stdinInstall) {
    return stdinInstall.exitCode === 0
      ? { exitCode: 0, stdout: "Success\n" }
      : {
          exitCode: stdinInstall.exitCode,
          stderr: stdinInstall.stderr || stdinInstall.stdout || "pm install failed without output",
        };
  }
  await pushBridgeFileViaSync(bytes, remotePath);
  const installCommand = adbShellCommandLine(["pm", "install", ...flags, remotePath]);
  log(`Install: ${installCommand}`);
  const stdout = await shellText(installCommand);
  const cleanupCommand = adbShellCommandLine(["rm", "-f", remotePath]);
  try {
    await shellText(cleanupCommand);
  } catch (error) {
    log(`Install cleanup failed: ${formatError(error)}`);
  }
  const success = /\bSuccess\b/.test(stdout);
  return success
    ? { exitCode: 0, stdout }
    : { exitCode: 1, stderr: stdout || "pm install failed without output" };
};

const bridgeProtocolResult = (result) => {
  const { ui, ...protocolResult } = result;
  return protocolResult;
};

const executeBridgeAdbCommand = async (command) => {
  const normalized = normalizedBridgeAdbArgs(command.args || []);
  const serial = elements.deviceSerial.textContent.trim();
  const model = elements.deviceModel.textContent.trim();
  if (normalized.length === 0 || normalized[0] === "devices") {
    return {
      exitCode: 0,
      stdout: `List of devices attached\n${serial}\tdevice product:${model || "Android"} model:${model || "Android"}\n`,
    };
  }
  if (normalized[0] === "get-state") {
    return { exitCode: 0, stdout: "device\n" };
  }
  if (normalized[0] === "get-serialno") {
    return { exitCode: 0, stdout: `${serial}\n` };
  }
  if (isBridgeInstallCommand(normalized)) {
    return executeBridgeInstallCommand(command, normalized);
  }
  if (normalized[0] === "shell") {
    const command = adbShellCommandLine(normalized.slice(1));
    const stdout = await shellText(command);
    elements.shellCommand.value = command;
    elements.shellOutput.textContent = `$ ${command}\n${stdout}`;
    return { exitCode: 0, stdout };
  }
  if (normalized[0] === "exec-out") {
    const command = adbShellCommandLine(normalized.slice(1));
    const bytes = await shellBytes(command);
    const ui = {};
    if (isBridgeScreenshotCommand(normalized)) {
      const display = displayScreenshotBytes(bytes, "Bridge screenshot captured");
      ui.screenshot = {
        size: display.size,
        url: URL.createObjectURL(new Blob([bytes], { type: "image/png" })),
      };
    }
    return { exitCode: 0, stdoutBase64: bytesToBase64(bytes), ui };
  }
  return {
    exitCode: 1,
    stderr: `Unsupported browser-bridge adb command: ${normalized.join(" ")}\nSupported: devices, get-state, get-serialno, install, shell, exec-out.\n`,
  };
};

const executeBridgeCommand = async (command) => {
  if (command.kind === "adb") {
    return executeBridgeAdbCommand(command);
  }
  return {
    exitCode: 1,
    stderr: `Unsupported bridge command kind: ${command.kind}\n`,
  };
};

const postBridgeResult = async (result) => {
  await fetch("/bridge/commands/result", {
    method: "POST",
    headers: bridgeHeaders({ "Content-Type": "application/json" }),
    body: JSON.stringify(result),
  });
};

const processNextBridgeCommand = async () => {
  if (!state.bridgePollingEnabled || state.bridgeBusy) {
    return;
  }
  state.bridgeBusy = true;
  try {
    const response = await fetch("/bridge/commands/next", {
      cache: "no-store",
      headers: bridgeHeaders(),
    });
    if (response.status === 409) {
      state.bridgePollingEnabled = false;
      state.bridgeClientId = null;
      await disconnectDevice();
      setMessage("Another browser tab owns this bridge session. Click Connect phone to take over.", "warn");
      log("Bridge client session lost to a newer browser tab");
      return;
    }
    if (!response.ok) {
      throw new Error(`bridge poll failed: HTTP ${response.status}`);
    }
    const payload = await response.json();
    const command = payload.command;
    if (!command) {
      return;
    }

    const activity = addBridgeActivity(command);
    log(`Bridge command ${command.id}: ${command.kind} ${(command.args || []).join(" ")}`);
    if (!state.adb) {
      const result = {
        exitCode: 1,
        stderr: "ADB is not connected. Reconnect the phone in the browser tab.\n",
        error: "ADB not connected",
      };
      await postBridgeResult({ id: command.id, ...result });
      updateBridgeActivity(activity, "bad", "failed", result.stderr);
      log(`Bridge command ${command.id} failed: ${result.error}`);
      return;
    }
    try {
      const result = await executeBridgeCommand(command);
      await postBridgeResult({ id: command.id, ...bridgeProtocolResult(result) });
      const ok = result.exitCode === 0 && !result.error;
      updateBridgeActivity(
        activity,
        ok ? "ok" : "bad",
        ok ? "done" : "failed",
        bridgeResultDetail(command, result),
        result.ui,
      );
      log(`Bridge command ${command.id} completed`);
    } catch (error) {
      await postBridgeResult({
        id: command.id,
        exitCode: 1,
        stderr: "",
        error: formatError(error),
      });
      updateBridgeActivity(activity, "bad", "failed", formatError(error));
      recordDiagnosticError(`Bridge command ${command.id} failed`, error);
    }
  } catch (error) {
    recordDiagnosticError("Bridge poll failed", error);
  } finally {
    state.bridgeBusy = false;
  }
};

const runShellCommand = async (event) => {
  event.preventDefault();
  const command = elements.shellCommand.value.trim();
  if (command.length === 0) {
    return;
  }
  try {
    elements.runShell.disabled = true;
    elements.shellOutput.textContent = `$ ${command}\n`;
    const output = await shellText(command);
    elements.shellOutput.textContent += output;
    log(`Shell command completed: ${command}`);
  } catch (error) {
    elements.shellOutput.textContent += formatError(error);
    log(`Shell command failed: ${formatError(error)}`);
  } finally {
    elements.runShell.disabled = !state.adb;
  }
};

const captureScreenshot = async () => {
  try {
    elements.captureScreen.disabled = true;
    log("Capturing screen");
    const bytes = await shellBytes("screencap -p");
    displayScreenshotBytes(bytes, "Screenshot captured");
  } catch (error) {
    log(`Screenshot failed: ${formatError(error)}`);
  } finally {
    elements.captureScreen.disabled = !state.adb;
  }
};

const formatError = (error) => {
  if (error instanceof Error) {
    return error.message;
  }
  return String(error);
};

const diagnosticsPayload = () => {
  const usbDevices = Array.from(elements.knownDevices.querySelectorAll("li")).map((item) =>
    item.textContent.trim(),
  );
  return {
    app: "rust-android-connection web",
    location: window.location.href,
    secureContext: window.isSecureContext,
    webUsbSupported: "usb" in navigator,
    packages: packageVersions,
    device: {
      serial: elements.deviceSerial.textContent,
      model: elements.deviceModel.textContent,
      android: elements.deviceAndroid.textContent,
      pill: elements.devicePill.textContent,
    },
    knownDevices: usbDevices,
    message: elements.connectionMessage.textContent,
    log: state.diagnostics,
  };
};

const copyDiagnostics = async () => {
  const payload = JSON.stringify(diagnosticsPayload(), null, 2);
  try {
    if (navigator.clipboard?.writeText) {
      await navigator.clipboard.writeText(payload);
    } else {
      const textarea = document.createElement("textarea");
      textarea.value = payload;
      textarea.setAttribute("readonly", "true");
      textarea.style.position = "fixed";
      textarea.style.top = "-1000px";
      document.body.append(textarea);
      textarea.select();
      document.execCommand("copy");
      textarea.remove();
    }
    setMessage("Diagnostics copied to clipboard", "ok");
    log("Diagnostics copied to clipboard");
  } catch (error) {
    setMessage(`Copy diagnostics failed: ${formatError(error)}`, "bad");
    recordDiagnosticError("Copy diagnostics failed", error);
  }
};

const initialize = async () => {
  await registerBridgeClient();
  const webUsbSupported = "usb" in navigator;
  setPill(
    elements.supportPill,
    webUsbSupported ? "WebUSB available" : "WebUSB unavailable",
    webUsbSupported ? "ok" : "bad",
  );
  setPill(
    elements.contextPill,
    window.isSecureContext ? "Secure context" : "Needs localhost or HTTPS",
    window.isSecureContext ? "ok" : "bad",
  );
  renderDisconnected();

  if (!webUsbSupported || !window.isSecureContext) {
    elements.refreshDevices.disabled = true;
    elements.connectDevice.disabled = true;
    elements.forgetDevices.disabled = true;
    renderKnownDevices([]);
    log("Use Chromium over localhost or HTTPS. Host ADB is not required.");
    return;
  }

  state.manager = AdbDaemonWebUsbDeviceManager.BROWSER;
  if (!state.manager) {
    elements.refreshDevices.disabled = true;
    elements.connectDevice.disabled = true;
    elements.forgetDevices.disabled = true;
    renderKnownDevices([]);
    log("WebUSB device manager is unavailable in this browser context.");
    return;
  }
  elements.refreshDevices.addEventListener("click", refreshKnownDevices);
  elements.connectDevice.addEventListener("click", connectDevice);
  elements.forgetDevices.addEventListener("click", forgetKnownDevices);
  elements.disconnectDevice.addEventListener("click", disconnectDevice);
  elements.copyDiagnostics.addEventListener("click", copyDiagnostics);
  elements.shellForm.addEventListener("submit", runShellCommand);
  elements.captureScreen.addEventListener("click", captureScreenshot);
  log(
    `Packages: ${packageVersions.adb}, ${packageVersions.webusb}, ${packageVersions.credential}`,
  );
  navigator.usb.addEventListener("connect", (event) => {
    log(`USB connected: ${event.device.productName || event.device.serialNumber || "device"}`);
  });
  navigator.usb.addEventListener("disconnect", (event) => {
    log(`USB disconnected: ${event.device.productName || event.device.serialNumber || "device"}`);
    if (state.backend?.raw === event.device) {
      renderDisconnected();
      setMessage("USB device disconnected", "warn");
    }
  });
  window.setInterval(processNextBridgeCommand, 750);
  await refreshKnownDevices();
};

initialize().catch((error) => {
  setPill(elements.supportPill, "Initialization failed", "bad");
  recordDiagnosticError("Initialization failed", error);
});

window.addEventListener("error", (event) => {
  recordDiagnosticError("Browser error", event.error || event.message);
});

window.addEventListener("unhandledrejection", (event) => {
  recordDiagnosticError("Unhandled promise rejection", event.reason);
});
