(function installTextDecoder(global, ops) {
  const states = new WeakMap();
  const arrayBufferLength = Object.getOwnPropertyDescriptor(ArrayBuffer.prototype, 'byteLength').get;
  const sharedBufferLength = typeof SharedArrayBuffer === 'function'
    ? Object.getOwnPropertyDescriptor(SharedArrayBuffer.prototype, 'byteLength').get : null;
  const stateFor = receiver => {
    const state = states.get(receiver);
    if (!state) throw new TypeError('Illegal invocation');
    return state;
  };
  global.TextDecoder = class TextDecoder {
    constructor(label = 'utf-8', options = {}) {
      options ??= {};
      const encoding = ops.op_encoding_for_label('' + label);
      if (!encoding || encoding === 'replacement') throw new RangeError('Invalid TextDecoder encoding label');
      const fatal = !!options.fatal, ignoreBOM = !!options.ignoreBOM;
      states.set(this, {encoding, fatal, ignoreBOM,
        native:ops.op_text_decoder_new(encoding, fatal, ignoreBOM)});
    }
    get encoding() { return stateFor(this).encoding; }
    get fatal() { return stateFor(this).fatal; }
    get ignoreBOM() { return stateFor(this).ignoreBOM; }
    decode(input, options = {}) {
      const state = stateFor(this);
      let bytes;
      if (input === undefined) bytes = new Uint8Array(0);
      else if (ArrayBuffer.isView(input)) bytes = new Uint8Array(input.buffer, input.byteOffset, input.byteLength);
      else {
        try { arrayBufferLength.call(input); }
        catch (_) {
          if (!sharedBufferLength) throw new TypeError('TextDecoder input must be a BufferSource');
          sharedBufferLength.call(input);
        }
        bytes = new Uint8Array(input);
      }
      return ops.op_text_decoder_decode(state.native, bytes, !!options?.stream);
    }
  };
  Object.defineProperty(global.TextDecoder.prototype, Symbol.toStringTag, {value:'TextDecoder', configurable:true});
})
