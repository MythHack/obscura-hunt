(function installWorkerMessaging(ops, config) {
  'use strict';
  const global = globalThis;
  const workerGlobal = config !== null;
  const workers = new Map();
  const portStates = new WeakMap();
  const listeners = new WeakMap();
  const handlerSlots = new WeakMap();
  let reportingError = false;
  function report(error) {
    if (!workerGlobal || reportingError) { if (global.console) console.error(error); return; }
    reportingError = true;
    try {
      const event = new ErrorEvent('error', { message: String(error?.message || error), filename: location.href, error, cancelable: true });
      if (dispatch.call(global, event)) call('op_worker_report_error', error, global);
    } finally { reportingError = false; }
  }
  function eventHandler(target, type, callback) {
    let slots = handlerSlots.get(target);
    if (!slots) handlerSlots.set(target, slots = new Map());
    const previous = slots.get(type);
    const value = typeof callback === 'function' || callback && typeof callback.handleEvent === 'function' ? callback : null;
    if (previous?.wrapper && value) { previous.value = value; return; }
    if (previous?.wrapper) remove.call(target, type, previous.wrapper);
    const slot = {value, wrapper: null};
    if (value) slot.wrapper = event => {
      const handler = slot.value;
      const globalError = workerGlobal && target === global && type === 'error';
      const result = typeof handler === 'function' ? globalError
        ? handler.call(target,event.message,event.filename,event.lineno,event.colno,event.error)
        : handler.call(target,event) : handler.handleEvent.call(handler,event);
      if (globalError ? result === true : result === false) event.preventDefault();
    };
    slots.set(type, slot);
    if (slot.wrapper) add.call(target, type, slot.wrapper);
  }
  function nativeError(error) {
    const name = String(error && error.name || 'Error');
    if (name.startsWith('DOMException')) return new DOMException(String(error.message), name.slice(12));
    // Shared native ops build their exception in the control realm. The
    // structured-clone binding must expose this realm's DOMException.
    return name === 'DataCloneError' && !(error instanceof DOMException)
      ? new DOMException(String(error.message), name) : error;
  }
  function call(name, ...args) {
    try { return ops[name](...args); } catch (error) { throw nativeError(error); }
  }
  function transferList(value) {
    const list = value == null ? [] : (Array.isArray(value) ? value : value.transfer || []);
    return Array.from(list);
  }
  function add(type, callback, options) {
    if (callback == null) return;
    type = String(type);
    let types = listeners.get(this);
    if (!types) listeners.set(this, types = new Map());
    let entries = types.get(type);
    if (!entries) types.set(type, entries = []);
    const capture = typeof options === 'boolean' ? options : !!options?.capture;
    if (!entries.some(entry => entry.callback === callback && entry.capture === capture)) {
      entries.push({ callback, capture, once: !!options?.once });
    }
  }
  function remove(type, callback, options) {
    const entries = listeners.get(this)?.get(String(type));
    const capture = typeof options === 'boolean' ? options : !!options?.capture;
    if (entries) {
      const index = entries.findIndex(entry => entry.callback === callback && entry.capture === capture);
      if (index !== -1) entries.splice(index, 1);
    }
  }
  function dispatch(event) {
    Object.defineProperties(event, { target: { value: this, configurable: true }, currentTarget: { value: this, configurable: true } });
    const entries = listeners.get(this)?.get(event.type) || [];
    // IDL event handlers occupy the same registration list as listeners.
    for (const entry of entries.slice()) {
      if (!entries.includes(entry)) continue;
      if (entry.once) remove.call(this, event.type, entry.callback, entry.capture);
      try {
        if (typeof entry.callback === 'function') entry.callback.call(this, event);
        else entry.callback.handleEvent.call(entry.callback, event);
      } catch (error) { report(error); }
    }
    return !event.defaultPrevented;
  }
  if (workerGlobal) {
    global.DOMException = class DOMException extends Error {
      constructor(message = '', name = 'Error') { super(String(message)); this.name = String(name); }
    };
    for (const name of ['SecurityError', 'SyntaxError']) {
      __obscuraCore.registerErrorBuilder('DOMException' + name, message => new DOMException(message, name));
    }
    global.Event = class Event {
      constructor(type, init = {}) { this.type = String(type); this.bubbles = !!init.bubbles; this.cancelable = !!init.cancelable; this.defaultPrevented = false; }
      preventDefault() { if (this.cancelable) this.defaultPrevented = true; }
      stopPropagation() {}
      stopImmediatePropagation() {}
    };
    global.MessageEvent = class MessageEvent extends Event {
      constructor(type, init = {}) { super(type, init); this.data = init.data; this.origin = init.origin || ''; this.source = init.source ?? null; this.ports = Object.freeze(Array.from(init.ports || [])); this.lastEventId = init.lastEventId || ''; }
    };
    global.ErrorEvent = class ErrorEvent extends Event {
      constructor(type, init = {}) { super(type, init); this.message = init.message || ''; this.filename = init.filename || ''; this.lineno = init.lineno || 0; this.colno = init.colno || 0; this.error = init.error ?? null; }
    };
    global.EventTarget = class EventTarget {
      addEventListener(...args) { return add.apply(this, args); }
      removeEventListener(...args) { return remove.apply(this, args); }
      dispatchEvent(event) { return dispatch.call(this, event); }
    };
  }
  for (const name of ['DataCloneError', 'NetworkError']) {
    __obscuraCore.registerErrorBuilder('DOMException' + name, message => new DOMException(message, name));
  }
  function wrapPort(port) {
    Object.setPrototypeOf(port, MessagePort.prototype);
    portStates.set(port, { onmessage: null, onmessageerror: null });
    return port;
  }
  function bind() {
    call('op_worker_bind', global, {
      port: wrapPort,
      message(kind, id, data, ports) {
        const target = kind === 'worker' ? global : kind === 'parent' ? workers.get(id) : id;
        if (!target) return;
        dispatch.call(target, new MessageEvent('message', { data, ports, origin: '', source: null }));
      },
      error(id, message, filename, lineno, colno) {
        const target = id === 0 ? global : workers.get(id);
        if (!target) return false;
        return !dispatch.call(target, new ErrorEvent('error', { message, filename, lineno, colno, cancelable: true }));
      },
      start: workerGlobal ? start : null,
      key(handle) { return global.__obscura_clone_hooks.NativeCryptoKey(handle); },
      platformKind(value) { return typeof global.Blob === 'function' && value instanceof global.Blob; },
      platformRead(value) { if (!(value instanceof global.Blob) || !(value._bytes instanceof Uint8Array)) return null; const file = typeof global.File === 'function' && value instanceof global.File; return { kind: file ? 1 : 0, bytes: value._bytes, mime: value.type, name: file ? value.name : '', modified: file ? value.lastModified : 0 }; },
      platformWrite(kind, bytes, mime, name, modified) { const value = Object.create(kind === 1 ? global.File.prototype : global.Blob.prototype); value._bytes = bytes; value.size = bytes.byteLength; value.type = mime; if (kind === 1) { value.name = name; value.lastModified = modified; } return value; },
    });
  }
  class MessagePort {
    constructor() { throw new TypeError('Illegal constructor'); }
    postMessage(data, options) { bind(); call('op_worker_port_send', this, data, transferList(options), global); }
    start() { bind(); call('op_worker_port_control', this, false, global); }
    close() { bind(); call('op_worker_port_control', this, true, global); }
    addEventListener(...args) { return add.apply(this, args); }
    removeEventListener(...args) { return remove.apply(this, args); }
    dispatchEvent(event) { return dispatch.call(this, event); }
    get onmessage() { const state = portStates.get(this); if (!state) throw new TypeError('Illegal invocation'); return state.onmessage; }
    set onmessage(value) { const state = portStates.get(this); if (!state) throw new TypeError('Illegal invocation'); state.onmessage = value; eventHandler(this, 'message', value); this.start(); }
    get onmessageerror() { const state = portStates.get(this); if (!state) throw new TypeError('Illegal invocation'); return state.onmessageerror; }
    set onmessageerror(value) { const state = portStates.get(this); if (!state) throw new TypeError('Illegal invocation'); state.onmessageerror = value; eventHandler(this, 'messageerror', value); }
    get [Symbol.toStringTag]() { return 'MessagePort'; }
  }
  class MessageChannel {
    constructor() { bind(); const pair = call('op_worker_port_pair', global); this.port1 = wrapPort(pair[0]); this.port2 = wrapPort(pair[1]); }
    get [Symbol.toStringTag]() { return 'MessageChannel'; }
  }
  global.MessagePort = MessagePort;
  global.MessageChannel = MessageChannel;
  global.structuredClone = function structuredClone(value, options) { bind(); return call('op_worker_clone', value, transferList(options), global); };
  global.Worker = class Worker {
    constructor(url, options = {}) {
      bind();
      this.onmessage = null; this.onmessageerror = null; this.onerror = null;
      const id = call('op_worker_create', String(url), String(options.name || ''), String(options.type || 'classic'), global);
      workers.set(id, this);
      workerIds.set(this, id);
    }
    postMessage(data, options) { const id = workerIds.get(this); if (id === undefined) throw new TypeError('Illegal invocation'); call('op_worker_send', id, data, transferList(options), global); }
    terminate() { const id = workerIds.get(this); if (id === undefined) throw new TypeError('Illegal invocation'); call('op_worker_terminate', id, global); workers.delete(id); }
    addEventListener(...args) { return add.apply(this, args); }
    removeEventListener(...args) { return remove.apply(this, args); }
    dispatchEvent(event) { return dispatch.call(this, event); }
  };
  const workerIds = new WeakMap();
  Object.setPrototypeOf(Worker.prototype, EventTarget.prototype);
  Object.setPrototypeOf(MessagePort.prototype, EventTarget.prototype);
  for (const prototype of [Worker.prototype]) for (const type of ['message', 'messageerror', 'error']) Object.defineProperty(prototype, 'on' + type, {
    get() { return handlerSlots.get(this)?.get(type)?.value || null; },
    set(value) { eventHandler(this, type, value); }, configurable: true, enumerable: true,
  });
  if (!workerGlobal) return;
  global.WorkerGlobalScope = class WorkerGlobalScope extends EventTarget { constructor() { throw new TypeError('Illegal constructor'); } };
  global.DedicatedWorkerGlobalScope = class DedicatedWorkerGlobalScope extends WorkerGlobalScope { constructor() { throw new TypeError('Illegal constructor'); } };
  Object.setPrototypeOf(global, DedicatedWorkerGlobalScope.prototype);
  Object.defineProperty(global, 'self', { value: global, enumerable: true });
  global.addEventListener = function addEventListener(type, callback, options = undefined) { return add.call(this ?? global, type, callback, options); };
  global.removeEventListener = function removeEventListener(type, callback, options = undefined) { return remove.call(this ?? global, type, callback, options); };
  global.dispatchEvent = function dispatchEvent(event) { return dispatch.call(this ?? global, event); };
  Object.defineProperty(global, 'name', { value: config.name, enumerable: true });
  const identity = config.identity || {};
  class WorkerNavigator { constructor() { throw new TypeError('Illegal constructor'); } }
  const language = identity.__obscura_language || 'en-US';
  const languages = Object.freeze((identity.__obscura_languages?.length ? identity.__obscura_languages : (language.includes('-') ? [language,language.split('-')[0]] : [language])).slice());
  const navigator = Object.create(WorkerNavigator.prototype);
  const navigatorValues = { userAgent: identity.__obscura_ua || 'Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/145.0.0.0 Safari/537.36', platform: identity.__obscura_platform || 'Win32', language, languages, onLine: true };
  for (const [key,value] of Object.entries(navigatorValues)) Object.defineProperty(WorkerNavigator.prototype,key,{get() { return value; },enumerable:true});
  Object.defineProperty(WorkerNavigator.prototype,Symbol.toStringTag,{value:'WorkerNavigator'});
  global.WorkerNavigator = WorkerNavigator;
  Object.defineProperty(global,'navigator',{value:navigator,enumerable:true});
  for (const type of ['message', 'messageerror', 'error']) Object.defineProperty(global, 'on' + type, {
    get() { return handlerSlots.get(global)?.get(type)?.value || null; },
    set(value) { eventHandler(global, type, value); }, configurable: true, enumerable: true,
  });
  class WorkerLocation {
    constructor() { if (locationCreated) throw new TypeError('Illegal constructor'); locationCreated = true; }
    toString() { return this.href; }
    get [Symbol.toStringTag]() { return 'WorkerLocation'; }
  }
  let locationCreated = false;
  let locationHref, locationParts;
  function currentLocation() { const href = call('op_worker_location', global); if (locationHref !== href) { locationParts = JSON.parse(call('op_url_parse', href, '')); locationHref = href; } return locationParts; }
  for (const key of ['href','origin','protocol','host','hostname','port','pathname','search','hash']) Object.defineProperty(WorkerLocation.prototype,key,{get() { return currentLocation()[key] || ''; },enumerable:true});
  global.WorkerLocation = WorkerLocation;
  Object.defineProperty(global, 'location', { value: new WorkerLocation(), enumerable: true });
  global.URL = class URL {
    constructor(input, base) { this._parts = JSON.parse(call('op_url_parse', String(input), base === undefined ? '' : String(base))); if (this._parts.error || !this._parts.href) throw new TypeError('Invalid URL'); }
    toString() { return this.href; }
    toJSON() { return this.href; }
    static createObjectURL(blob) { if (!(blob instanceof Blob)) throw new TypeError('Expected Blob'); return call('op_object_url_create', blob._bytes, blob.type, global); }
    static revokeObjectURL(url) { call('op_object_url_revoke', String(url), global); }
  };
  for (const key of ['href','origin','protocol','host','hostname','port','pathname','search','hash']) Object.defineProperty(URL.prototype, key, { get() { return this._parts[key] || ''; } });
  global.TextEncoder = class TextEncoder {
    get encoding() { return 'utf-8'; }
    encode(value = '') { const bytes = []; for (let char of String(value)) { let n = char.codePointAt(0); if (n >= 0xd800 && n <= 0xdfff) n = 0xfffd; if (n < 128) bytes.push(n); else if (n < 2048) bytes.push(192 | n >> 6, 128 | n & 63); else if (n < 65536) bytes.push(224 | n >> 12, 128 | n >> 6 & 63, 128 | n & 63); else bytes.push(240 | n >> 18, 128 | n >> 12 & 63, 128 | n >> 6 & 63, 128 | n & 63); } return new Uint8Array(bytes); }
    encodeInto(value, destination) { let read = 0, written = 0; for (const character of String(value)) { const bytes = this.encode(character); if (written + bytes.length > destination.length) break; destination.set(bytes, written); written += bytes.length; read += character.length; } return { read, written }; }
  };
  global.Blob = class Blob {
    constructor(parts = [], options = {}) { const arrays = Array.from(parts, part => part instanceof Blob ? part._bytes : ArrayBuffer.isView(part) ? new Uint8Array(part.buffer, part.byteOffset, part.byteLength) : part instanceof ArrayBuffer ? new Uint8Array(part) : new TextEncoder().encode(String(part))); this._bytes = new Uint8Array(arrays.reduce((size, part) => size + part.length, 0)); let offset = 0; for (const part of arrays) { this._bytes.set(part, offset); offset += part.length; } this.type = String(options.type || '').toLowerCase(); this.size = this._bytes.length; }
    async arrayBuffer() { return this._bytes.slice().buffer; }
    async text() { return new TextDecoder().decode(this._bytes); }
    slice(start, end, type = '') { return new Blob([this._bytes.slice(start, end)], { type }); }
    bytes() { return Promise.resolve(this._bytes.slice()); }
    stream() { const bytes = this._bytes; return new ReadableStream({start(controller) { controller.enqueue(bytes.slice()); controller.close(); }}); }
    get [Symbol.toStringTag]() { return 'Blob'; }
  };
  global.File = class File extends Blob {
    constructor(parts,name,options = {}) { if (arguments.length < 2) throw new TypeError('File requires parts and name'); super(parts,options); this.name = String(name); this.lastModified = options.lastModified === undefined ? Date.now() : Number(options.lastModified); }
    get [Symbol.toStringTag]() { return 'File'; }
  };
  global.Headers = class Headers {
    constructor(init = {}) { this._values = new Map(); for (const [key, value] of init instanceof Headers ? init : Array.isArray(init) ? init : Object.entries(init)) this.append(key, value); }
    append(key, value) { key = String(key).toLowerCase(); this._values.set(key, this._values.has(key) ? this._values.get(key) + ', ' + value : String(value)); }
    set(key, value) { this._values.set(String(key).toLowerCase(), String(value)); }
    get(key) { return this._values.get(String(key).toLowerCase()) ?? null; }
    has(key) { return this._values.has(String(key).toLowerCase()); }
    delete(key) { this._values.delete(String(key).toLowerCase()); }
    entries() { return this._values.entries(); }
    keys() { return this._values.keys(); }
    values() { return this._values.values(); }
    forEach(callback, receiver) { this._values.forEach((value, key) => callback.call(receiver, value, key, this)); }
    [Symbol.iterator]() { return this.entries(); }
  };
  function decodeBase64(input) { const alphabet = 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/'; let bits = 0, value = 0; const output = []; for (const char of input.replace(/\s/g, '')) { if (char === '=') break; const digit = alphabet.indexOf(char); if (digit < 0) throw new DOMException('Invalid base64', 'InvalidCharacterError'); value = value << 6 | digit; bits += 6; if (bits >= 8) { bits -= 8; output.push(value >> bits & 255); } } return new Uint8Array(output); }
  global.atob = input => String.fromCharCode(...decodeBase64(String(input)));
  global.btoa = input => { const alphabet = 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/'; const text = String(input); let output = ''; for (let i = 0; i < text.length; i += 3) { const a = text.charCodeAt(i), b = text.charCodeAt(i + 1), c = text.charCodeAt(i + 2); if (a > 255 || b > 255 || c > 255) throw new DOMException('Invalid character', 'InvalidCharacterError'); const n = a << 16 | (b || 0) << 8 | (c || 0); output += alphabet[n >>> 18] + alphabet[n >>> 12 & 63] + (i + 1 < text.length ? alphabet[n >>> 6 & 63] : '=') + (i + 2 < text.length ? alphabet[n & 63] : '='); } return output; };
  class Response {
    constructor(metadata, bytes) {
      this.status = metadata.status; this.statusText = metadata.statusText || ''; this.url = metadata.url || '';
      this.headers = new Headers(metadata.headers || {}); this.ok = this.status >= 200 && this.status < 300; this.bodyUsed = false;
      this._bytes = bytes || (metadata.bodyRid != null ? ops.op_fetch_body(metadata.bodyRid, global).then(value => new Uint8Array(value))
        : Promise.resolve(metadata.bodyBytes != null ? new Uint8Array(metadata.bodyBytes) : new TextEncoder().encode(metadata.body || '')));
      this.body = new ReadableStream({ start: controller => { this._bytes.then(bytes => { controller.enqueue(bytes.slice()); controller.close(); }, error => controller.error(error)); } });
    }
    async arrayBuffer() { if (this.bodyUsed) throw new TypeError('Body already consumed'); this.bodyUsed = true; return (await this._bytes).slice().buffer; }
    async text() { return new TextDecoder().decode(await this.arrayBuffer()); }
    async json() { return JSON.parse(await this.text()); }
    async blob() { return new Blob([await this.arrayBuffer()], { type: this.headers.get('content-type') || '' }); }
    clone() { if (this.bodyUsed) throw new TypeError('Body already consumed'); return new Response({ status: this.status, statusText: this.statusText, url: this.url, headers: Object.fromEntries(this.headers) }, this._bytes); }
  }
  global.Response = Response;
  global.fetch = async function fetch(input, init = {}) {
    const url = new URL(String(input?.url || input), location.href).href;
    const headers = JSON.stringify(Object.fromEntries(new Headers(init.headers || input?.headers || {})));
    const body = init.body == null ? new Uint8Array() : init.body instanceof Blob ? init.body._bytes : ArrayBuffer.isView(init.body) ? new Uint8Array(init.body.buffer, init.body.byteOffset, init.body.byteLength) : init.body instanceof ArrayBuffer ? new Uint8Array(init.body) : new TextEncoder().encode(String(init.body));
    try { const result = await ops.op_fetch_url(url, String(init.method || input?.method || 'GET'), headers, body, '', String(init.mode || 'cors'), String(init.credentials || 'same-origin'), { destination: 'Fetch', redirect: init.redirect || 'follow' }, global); return new Response(JSON.parse(result)); } catch (error) { throw nativeError(error); }
  };
  global.postMessage = (data, options) => call('op_worker_send', 0, data, transferList(options), global);
  global.close = () => call('op_worker_terminate', 0, global);
  global.queueMicrotask = callback => { if (typeof callback !== 'function') throw new TypeError('Expected callback'); Promise.resolve().then(callback); };
  global.setTimeout = (callback, delay = 0, ...args) => call('op_worker_timer', () => typeof callback === 'function' ? callback(...args) : call('op_worker_eval', String(callback), location.href, global), Math.max(0, Math.min(2147483647, Number(delay) || 0)), false, global);
  global.setInterval = (callback, delay = 0, ...args) => call('op_worker_timer', () => typeof callback === 'function' ? callback(...args) : call('op_worker_eval', String(callback), location.href, global), Math.max(1, Math.min(2147483647, Number(delay) || 0)), true, global);
  global.clearTimeout = global.clearInterval = id => call('op_worker_clear_timer', Number(id) >>> 0, global);
  global.performance = { now: () => call('op_worker_now'), timeOrigin: Date.now() };
  global.console = Object.fromEntries(['log','info','debug','warn','error'].map(level => [level, (...values) => call('op_worker_console', level, values.map(String).join(' '))]));
  global.crypto = { getRandomValues(array) { if (!ArrayBuffer.isView(array) || array instanceof Float32Array || array instanceof Float64Array) throw new TypeError('Expected integer typed array'); if (array.byteLength > 65536) throw new DOMException('Quota exceeded', 'QuotaExceededError'); call('op_worker_random', new Uint8Array(array.buffer, array.byteOffset, array.byteLength)); return array; }, randomUUID() { const bytes = new Uint8Array(16); this.getRandomValues(bytes); bytes[6] = bytes[6] & 15 | 64; bytes[8] = bytes[8] & 63 | 128; const hex = Array.from(bytes, b => b.toString(16).padStart(2, '0')).join(''); return `${hex.slice(0,8)}-${hex.slice(8,12)}-${hex.slice(12,16)}-${hex.slice(16,20)}-${hex.slice(20)}`; } };
  global.importScripts = function importScripts(...urls) {
    if (config.type === 'module') throw new TypeError('Module Workers cannot use importScripts');
    const parsed = urls.map(value => {
      try { return new URL(String(value), location.href).href; }
      catch (_) { throw new DOMException('Invalid script URL', 'SyntaxError'); }
    });
    for (const url of parsed) {
      let response;
      const local = call('op_worker_local_script', url, global);
      if (local !== null) { call('op_worker_eval', local, url, global); continue; }
      call('op_worker_script_policy', url, global);
      try { response = JSON.parse(ops.op_fetch_url_sync(url, 'GET', '{}', new Uint8Array(), '', 'no-cors', 'same-origin', 'Script', global)); } catch (error) { throw nativeError(error); }
      evaluateResponse(response, url);
    }
  };
  function evaluateResponse(response, url) {
    if (response.status < 200 || response.status >= 300) throw new DOMException('Worker script HTTP ' + response.status, 'NetworkError');
    const mime = String(response.headers?.['content-type'] || '').split(';')[0].trim().toLowerCase();
    if (!/^(?:application|text)\/(?:x-)?(?:java|ecma)script$/.test(mime) && !/^text\/(?:javascript1\.[0-5]|jscript|livescript)$/.test(mime)) throw new DOMException('Worker script has an unsupported MIME type', 'NetworkError');
    const source = response.bodyBytes != null ? new TextDecoder().decode(new Uint8Array(response.bodyBytes)) : String(response.body || '');
    call('op_worker_eval', source, response.url || url, global);
  }
  async function start() {
    try {
      if (config.type === 'module') { await import(config.url); call('op_worker_ready', global); return; }
      const local = call('op_worker_local_script', config.url, global);
      if (local !== null) call('op_worker_eval', local, config.url, global);
      else {
        const result = await ops.op_fetch_url(config.url, 'GET', '{}', new Uint8Array(), '', 'same-origin', 'same-origin', 'Script', global);
        evaluateResponse(JSON.parse(result), config.url);
      }
      call('op_worker_ready', global);
    } catch (error) { call('op_worker_failed', error, global); }
  }
  bind();
})
