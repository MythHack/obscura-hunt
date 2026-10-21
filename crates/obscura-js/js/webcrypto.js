(function () {
if (!globalThis.crypto) globalThis.crypto = {};
if (!globalThis.crypto.subtle) {
  // Public-key material and policy live in GC-owned native handles. Secret-key
  // primitives retain their existing RustCrypto path. Neither is an author
  // property of CryptoKey, and public metadata never authorizes an operation.
  const keyMaterial = new WeakMap();
  const publicAlgorithms = new Set(['RSA-OAEP','RSASSA-PKCS1-v1_5','RSA-PSS','ECDSA','ECDH','Ed25519','X25519']);
  const wrappingAlgorithms = new Set(['RSA-OAEP','AES-KW','AES-GCM','AES-CBC','AES-CTR']);
  const toBytes = data => {
    if (data instanceof ArrayBuffer) return new Uint8Array(data);
    if (ArrayBuffer.isView(data)) return new Uint8Array(data.buffer,data.byteOffset,data.byteLength);
    throw new TypeError('Expected a BufferSource');
  };
  const bufferOf = bytes => new Uint8Array(bytes).buffer;
  const empty = () => new Uint8Array(0);
  class CryptoKey {
    constructor() { throw new TypeError('Illegal constructor'); }
    get [Symbol.toStringTag]() { return 'CryptoKey'; }
  }
  const usageOrder = ['encrypt','decrypt','sign','verify','deriveKey','deriveBits','wrapKey','unwrapKey'];
  function keyUsagesList(values) {
    const list = Array.from(values,String);
    if (list.some(u => !usageOrder.includes(u))) throw new TypeError('Invalid KeyUsage value');
    return list;
  }
  function keyFormat(value) {
    const format = String(value);
    if (!['raw','spki','pkcs8','jwk'].includes(format)) throw new TypeError('Invalid KeyFormat value');
    return format;
  }
  function copyAlgorithm(algorithm) {
    const copy = Object.assign({},algorithm);
    if (algorithm.hash) copy.hash = Object.assign({},algorithm.hash);
    if (algorithm.publicExponent) copy.publicExponent = new Uint8Array(algorithm.publicExponent);
    return copy;
  }
  function makeKey(type,extractable,algorithm,usages,bytes,handle) {
    handle = handle || runOp(() => __obscuraCore.ops.op_secret_key_new(bytes,algorithm,!!extractable,usages));
    const key = handle;
    Object.setPrototypeOf(key,CryptoKey.prototype);
    const policy = {type,extractable:!!extractable,algorithm:copyAlgorithm(algorithm),usages:usageOrder.filter(u => usages.includes(u)),bytes,handle};
    Object.defineProperties(key,{
      type:{value:type,enumerable:true}, extractable:{value:policy.extractable,enumerable:true},
      algorithm:{value:copyAlgorithm(algorithm),enumerable:true},
      usages:{value:Object.freeze(policy.usages.slice()),enumerable:true},
    });
    keyMaterial.set(key,policy);
    return key;
  }
  function nativeKey(handle) {
    const meta = runOp(() => __obscuraCore.ops.op_public_key_metadata(handle));
    const bytes = meta.type === 'secret' ? runOp(() => __obscuraCore.ops.op_secret_key_bytes(handle)) : undefined;
    if (meta.algorithm.publicExponent) meta.algorithm.publicExponent = new Uint8Array(meta.algorithm.publicExponent);
    return makeKey(meta.type,meta.extractable,meta.algorithm,meta.usages,bytes,handle);
  }
  function material(key,name,usage,type) {
    const policy = keyMaterial.get(key);
    if (!policy) throw new TypeError('Expected a CryptoKey');
    if ((name && policy.algorithm.name !== name) || (usage && !policy.usages.includes(usage)) || (type && policy.type !== type)) {
      throw new DOMException('The key does not permit this operation','InvalidAccessError');
    }
    return policy;
  }
  if (typeof _idbPlatformCodecs !== 'undefined') _idbPlatformCodecs.push({
    name:'CryptoKey',test:key => keyMaterial.has(key),
    encode:key => {
      const m = material(key);
      return {payload:runOp(() => __obscuraCore.ops.op_public_key_serialize(m.handle))};
    },
    decode:data => nativeKey(runOp(() => __obscuraCore.ops.op_public_key_deserialize(data.payload))),
  });
  globalThis.__obscura_clone_hooks = globalThis.__obscura_clone_hooks || {};
  globalThis.__obscura_clone_hooks.NativeCryptoKey = nativeKey;
  globalThis.__obscura_clone_hooks.CryptoKey = (source,seen) => {
    if (seen && seen.has(source)) return seen.get(source);
    const m = material(source);
    const copy = nativeKey(runOp(() => __obscuraCore.ops.op_public_key_clone(m.handle)));
    if (seen) seen.set(source,copy);
    return copy;
  };
  const ALGO_CANON = {
    'AES-CTR':'AES-CTR','AES-CBC':'AES-CBC','AES-GCM':'AES-GCM','AES-KW':'AES-KW',
    HMAC:'HMAC',PBKDF2:'PBKDF2',HKDF:'HKDF',
    'RSASSA-PKCS1-V1_5':'RSASSA-PKCS1-v1_5','RSA-PSS':'RSA-PSS','RSA-OAEP':'RSA-OAEP',
    ECDSA:'ECDSA',ECDH:'ECDH',ED25519:'Ed25519',X25519:'X25519',
  };
  function normalizeAlgo(algorithm) {
    const a = typeof algorithm === 'string' ? {name:algorithm} : (algorithm || {});
    if (a.name === undefined) throw new TypeError('Algorithm.name is required');
    const name = ALGO_CANON[String(a.name).toUpperCase()];
    if (!name) throw new DOMException('Unrecognized algorithm name','NotSupportedError');
    return Object.assign({},a,{name});
  }
  function normalizeHash(hash) {
    if (hash === undefined || typeof hash !== 'string' && (!hash || hash.name === undefined)) throw new TypeError('Hash.name is required');
    const name = String(typeof hash === 'string' ? hash : (hash && hash.name) || '').toUpperCase();
    if (['SHA-1','SHA-256','SHA-384','SHA-512'].includes(name)) return name;
    throw new DOMException('Unsupported hash algorithm','NotSupportedError');
  }
  const hashBlockSize = hash => hash === 'SHA-384' || hash === 'SHA-512' ? 128 : 64;
  function unsigned(value,name) {
    const n = Math.trunc(Number(value));
    if (!Number.isFinite(n) || n < 0 || n > 0xffffffff) throw new TypeError(name + ' is outside the unsigned long range');
    return n;
  }
  function parameters(alg,operation) {
    const p = {name:alg.name};
    if (alg.name.startsWith('RSA') && (operation === 'generate' || operation === 'import')) p.hash = normalizeHash(alg.hash);
    if ((alg.name === 'ECDSA' || alg.name === 'ECDH') && (operation === 'generate' || operation === 'import')) {
      if (alg.namedCurve === undefined) throw new TypeError('namedCurve is required');
      p.namedCurve = String(alg.namedCurve);
      if (!['P-256','P-384','P-521'].includes(p.namedCurve)) throw new DOMException('Unsupported named curve','NotSupportedError');
    }
    if (operation === 'generate' && alg.name.startsWith('RSA')) {
      p.modulusLength = unsigned(alg.modulusLength,'modulusLength');
      if (!(alg.publicExponent instanceof Uint8Array)) throw new TypeError('publicExponent must be a Uint8Array');
      if (alg.publicExponent.byteLength > 65536) throw new DOMException('RSA exponent exceeds the input limit','OperationError');
      p.publicExponent = Array.from(toBytes(alg.publicExponent));
    }
    if (operation === 'sign' || operation === 'verify') {
      if (alg.name === 'ECDSA') p.hash = normalizeHash(alg.hash);
      if (alg.name === 'RSA-PSS') {
        if (alg.saltLength === undefined) throw new TypeError('saltLength is required');
        p.saltLength = unsigned(alg.saltLength,'saltLength');
      }
    }
    return p;
  }
  function nativeError(error) {
    if (error instanceof DOMException || error instanceof TypeError) return error;
    const message = String(error && error.message || error);
    const match = /^(NotSupportedError|InvalidAccessError|SyntaxError|DataError|OperationError|TypeError):\s*([\s\S]*)$/.exec(message);
    if (match && match[1] === 'TypeError') return new TypeError(match[2]);
    return new DOMException(match ? match[2] : message,match ? match[1] : 'OperationError');
  }
  function runOp(fn) { try { return fn(); } catch (error) { throw nativeError(error); } }
  async function runAsyncOp(fn) { try { return await fn(); } catch (error) { throw nativeError(error); } }
  function b64urlToBytes(text) {
    if (typeof text !== 'string' || !/^[A-Za-z0-9_-]*$/.test(text)) throw new DOMException('Invalid base64url key','DataError');
    let s = text.replace(/-/g,'+').replace(/_/g,'/');
    while (s.length % 4) s += '=';
    try { return Uint8Array.from(atob(s),c => c.charCodeAt(0)); }
    catch (_) { throw new DOMException('Invalid base64url key','DataError'); }
  }
  function bytesToB64url(bytes) {
    let bin = '';
    for (let i=0;i<bytes.length;i++) bin += String.fromCharCode(bytes[i]);
    return btoa(bin).replace(/\+/g,'-').replace(/\//g,'_').replace(/=+$/,'');
  }
  // A JsonWebKey dictionary, not JSON.stringify(keyData). Unknown members and
  // author toJSON hooks never participate in native import/serialization.
  function jwkDictionary(value) {
    if (!value || typeof value !== 'object') throw new TypeError('Expected a JsonWebKey dictionary');
    const copy = Object.create(null);
    for (const name of ['alg','crv','d','dp','dq','e','k','kty','n','p','q','qi','use','x','y']) {
      if (value[name] !== undefined) {
        copy[name] = String(value[name]);
        if (copy[name].length > 131072) throw new DOMException('JWK member exceeds the native import limit','DataError');
      }
    }
    if (value.ext !== undefined) copy.ext = !!value.ext;
    if (value.key_ops !== undefined) copy.key_ops = Array.from(value.key_ops,String);
    if (value.oth !== undefined) copy.oth = [];
    return copy;
  }
  function secretUsages(name,requested) {
    const allowed = name === 'HMAC' ? ['sign','verify'] : name === 'PBKDF2' || name === 'HKDF' ? ['deriveBits','deriveKey'] : name === 'AES-KW' ? ['wrapKey','unwrapKey'] : ['encrypt','decrypt','wrapKey','unwrapKey'];
    if (!requested.length || requested.some(u => !allowed.includes(u))) throw new DOMException('Unsupported key usage','SyntaxError');
  }
  function keyAlgorithmFor(alg,bytes) {
    if (alg.name === 'HMAC') {
      if (!bytes.length) throw new DOMException('HMAC key cannot be empty','DataError');
      return {name:'HMAC',hash:{name:normalizeHash(alg.hash)},length:bytes.length*8};
    }
    if (alg.name.startsWith('AES-')) {
      if (![16,24,32].includes(bytes.length)) throw new DOMException('AES key data must be 128, 192, or 256 bits','DataError');
      return {name:alg.name,length:bytes.length*8};
    }
    if (alg.name === 'PBKDF2' || alg.name === 'HKDF') return {name:alg.name};
    throw new DOMException('Unsupported secret-key algorithm','NotSupportedError');
  }
  function jwkAlg(key,bytes) {
    const alg = material(key).algorithm;
    if (alg.name.startsWith('AES-')) return 'A' + bytes.length*8 + alg.name.slice(4);
    if (alg.name === 'HMAC') return 'HS' + alg.hash.name.slice(4);
    return undefined;
  }
  function exportBytes(format,key) {
    const m = material(key);
    if (!m.extractable) throw new DOMException('Key is not extractable','InvalidAccessError');
    if (m.type !== 'secret') return runOp(() => __obscuraCore.ops.op_public_key_export(m.handle,format));
    if (format === 'raw') return m.bytes;
    if (format === 'jwk') return __obscuraCore.ops.op_subtle_jwk_json(jwkAlg(key,m.bytes) || '',m.extractable,bytesToB64url(m.bytes),m.usages);
    throw new DOMException('Unsupported secret-key format','NotSupportedError');
  }
  function cipher(encrypt,algorithm,key,data,usage) {
    const alg = normalizeAlgo(algorithm);
    if (!['RSA-OAEP','AES-GCM','AES-CBC','AES-CTR'].includes(alg.name)) throw new DOMException('Unsupported encryption algorithm','NotSupportedError');
    const m = material(key,alg.name,usage || (encrypt ? 'encrypt' : 'decrypt'));
    const input = toBytes(data);
    if (alg.name === 'RSA-OAEP') {
      const label = alg.label === undefined ? empty() : toBytes(alg.label);
      return bufferOf(runOp(() => __obscuraCore.ops.op_public_key_cipher(m.handle,encrypt,usage === 'wrapKey' || usage === 'unwrapKey',label,input)));
    }
    if (alg.name === 'AES-GCM') {
      const iv = toBytes(alg.iv), aad = alg.additionalData == null ? empty() : toBytes(alg.additionalData);
      if (alg.tagLength != null && alg.tagLength !== 128) throw new DOMException('Only 128-bit AES-GCM tags are supported','NotSupportedError');
      return bufferOf(runOp(() => __obscuraCore.ops.op_subtle_aes_gcm(encrypt,m.bytes,iv,aad,input)));
    }
    if (alg.name === 'AES-CBC') return bufferOf(runOp(() => __obscuraCore.ops.op_subtle_aes_cbc(encrypt,m.bytes,toBytes(alg.iv),input)));
    return bufferOf(runOp(() => __obscuraCore.ops.op_subtle_aes_ctr(m.bytes,toBytes(alg.counter),unsigned(alg.length,'length'),input)));
  }
  function derive(algorithm,baseKey,length,usage) {
    const alg = normalizeAlgo(algorithm);
    if (!['ECDH','X25519','PBKDF2','HKDF'].includes(alg.name)) throw new DOMException('Unsupported derivation algorithm','NotSupportedError');
    const m = material(baseKey,alg.name,usage);
    const bits = length == null ? null : unsigned(length,'length');
    if (alg.name === 'ECDH' || alg.name === 'X25519') {
      const peer = material(alg.public,alg.name,undefined,'public');
      return bufferOf(runOp(() => __obscuraCore.ops.op_public_key_derive(m.handle,peer.handle,usage === 'deriveKey',{name:alg.name,length:bits})));
    }
    if (bits == null || bits === 0 || bits % 8) throw new DOMException('Invalid derived bit length','OperationError');
    if (alg.name === 'PBKDF2') return bufferOf(runOp(() => __obscuraCore.ops.op_subtle_pbkdf2(normalizeHash(alg.hash),m.bytes,toBytes(alg.salt),unsigned(alg.iterations,'iterations'),bits/8)));
    if (alg.name === 'HKDF') return bufferOf(runOp(() => __obscuraCore.ops.op_subtle_hkdf(normalizeHash(alg.hash),m.bytes,toBytes(alg.salt),toBytes(alg.info),bits/8)));
    throw new DOMException('Unsupported derivation algorithm','NotSupportedError');
  }
  const subtle = {
    async digest(algorithm,data) {
      const name = String(typeof algorithm === 'string' ? algorithm : algorithm && algorithm.name || '').toUpperCase();
      if (!['SHA-1','SHA-256','SHA-384','SHA-512','SHA-512/224','SHA-512/256'].includes(name)) throw new DOMException('Unrecognized algorithm name','NotSupportedError');
      return bufferOf(__obscuraCore.ops.op_subtle_digest(name,toBytes(data)));
    },
    async importKey(format,keyData,algorithm,extractable,keyUsages) {
      format = keyFormat(format);
      const alg = normalizeAlgo(algorithm), uses = keyUsagesList(keyUsages);
      if (publicAlgorithms.has(alg.name)) {
        const jwk = format === 'jwk' ? jwkDictionary(keyData) : null;
        const handle = runOp(() => __obscuraCore.ops.op_public_key_import(format,format === 'jwk' ? empty() : toBytes(keyData),jwk,parameters(alg,'import'),!!extractable,uses));
        return nativeKey(handle);
      }
      secretUsages(alg.name,uses);
      let bytes;
      if (format === 'raw') bytes = toBytes(keyData).slice();
      else if (format === 'jwk') {
        const jwk = jwkDictionary(keyData);
        if (jwk.kty !== 'oct' || typeof jwk.k !== 'string') throw new DOMException('Expected an oct JWK','DataError');
        if (jwk.ext === false && extractable || jwk.key_ops && (new Set(jwk.key_ops).size !== jwk.key_ops.length || uses.some(u => !jwk.key_ops.includes(u)))) throw new DOMException('JWK policy mismatch','DataError');
        bytes = b64urlToBytes(jwk.k);
        const temporary = makeKey('secret',extractable,keyAlgorithmFor(alg,bytes),uses,bytes);
        if (jwk.alg !== undefined && jwk.alg !== jwkAlg(temporary,bytes)) throw new DOMException('JWK algorithm mismatch','DataError');
        if (jwk.use !== undefined && jwk.use !== (alg.name === 'HMAC' ? 'sig' : 'enc')) throw new DOMException('JWK use mismatch','DataError');
      } else throw new DOMException('Unsupported secret-key format','NotSupportedError');
      if ((alg.name === 'PBKDF2' || alg.name === 'HKDF') && extractable) throw new DOMException('Derivation base keys are not extractable','SyntaxError');
      return makeKey('secret',extractable,keyAlgorithmFor(alg,bytes),uses,bytes);
    },
    async exportKey(format,key) {
      format = keyFormat(format);
      const bytes = exportBytes(format,key);
      return format === 'jwk' ? runOp(() => __obscuraCore.ops.op_subtle_jwk_parse(bytes)) : bufferOf(bytes);
    },
    async generateKey(algorithm,extractable,keyUsages) {
      const alg = normalizeAlgo(algorithm), uses = keyUsagesList(keyUsages);
      if (publicAlgorithms.has(alg.name)) {
        const handle = await runAsyncOp(() => __obscuraCore.ops.op_public_key_generate(parameters(alg,'generate'),!!extractable,uses));
        return {privateKey:nativeKey(handle),publicKey:nativeKey(runOp(() => __obscuraCore.ops.op_public_key_public(handle,uses)))};
      }
      if (alg.name === 'HMAC') {
        secretUsages(alg.name,uses);
        const hash = normalizeHash(alg.hash), bits = alg.length === undefined ? hashBlockSize(hash)*8 : unsigned(alg.length,'length');
        if (!bits || bits % 8) throw new DOMException('Invalid HMAC key length','OperationError');
        const bytes = runOp(() => __obscuraCore.ops.op_random_bytes(bits/8));
        return makeKey('secret',extractable,{name:'HMAC',hash:{name:hash},length:bits},uses,bytes);
      }
      if (alg.name.startsWith('AES-')) {
        secretUsages(alg.name,uses);
        if (![128,192,256].includes(alg.length)) throw new DOMException('Invalid AES key length','OperationError');
        const bytes = __obscuraCore.ops.op_random_bytes(alg.length/8);
        return makeKey('secret',extractable,{name:alg.name,length:alg.length},uses,bytes);
      }
      throw new DOMException('Unsupported key generation algorithm','NotSupportedError');
    },
    async sign(algorithm,key,data) {
      const alg = normalizeAlgo(algorithm);
      if (!['HMAC','RSASSA-PKCS1-v1_5','RSA-PSS','ECDSA','Ed25519'].includes(alg.name)) throw new DOMException('Unsupported signing algorithm','NotSupportedError');
      const m = material(key,alg.name,'sign');
      if (m.type !== 'secret') return bufferOf(runOp(() => __obscuraCore.ops.op_public_key_sign(m.handle,parameters(alg,'sign'),toBytes(data))));
      return bufferOf(runOp(() => __obscuraCore.ops.op_subtle_hmac(m.algorithm.hash.name,m.bytes,toBytes(data))));
    },
    async verify(algorithm,key,signature,data) {
      const alg = normalizeAlgo(algorithm);
      if (!['HMAC','RSASSA-PKCS1-v1_5','RSA-PSS','ECDSA','Ed25519'].includes(alg.name)) throw new DOMException('Unsupported verification algorithm','NotSupportedError');
      const m = material(key,alg.name,'verify');
      if (m.type !== 'secret') return runOp(() => __obscuraCore.ops.op_public_key_verify(m.handle,parameters(alg,'verify'),toBytes(signature),toBytes(data)));
      const mac = runOp(() => __obscuraCore.ops.op_subtle_hmac(m.algorithm.hash.name,m.bytes,toBytes(data))), sig = toBytes(signature);
      if (mac.length !== sig.length) return false;
      let difference = 0; for (let i=0;i<mac.length;i++) difference |= mac[i]^sig[i];
      return difference === 0;
    },
    async encrypt(algorithm,key,data) { return cipher(true,algorithm,key,data); },
    async decrypt(algorithm,key,data) { return cipher(false,algorithm,key,data); },
    async deriveBits(algorithm,baseKey,length) { return derive(algorithm,baseKey,length,'deriveBits'); },
    async deriveKey(algorithm,baseKey,derivedKeyAlgorithm,extractable,keyUsages) {
      const alg = normalizeAlgo(derivedKeyAlgorithm);
      let bits;
      if (alg.name === 'HMAC') bits = alg.length === undefined ? hashBlockSize(normalizeHash(alg.hash))*8 : unsigned(alg.length,'length');
      else if (alg.name.startsWith('AES-')) {
        bits = alg.length;
        if (![128,192,256].includes(bits)) throw new DOMException('Invalid derived AES key length','OperationError');
      } else throw new DOMException('Unsupported derived-key algorithm','NotSupportedError');
      const bytes = derive(algorithm,baseKey,bits,'deriveKey');
      return subtle.importKey('raw',bytes,derivedKeyAlgorithm,extractable,keyUsages);
    },
    async wrapKey(format,key,wrappingKey,wrapAlgorithm) {
      format = keyFormat(format);
      const alg = normalizeAlgo(wrapAlgorithm);
      if (!wrappingAlgorithms.has(alg.name)) throw new DOMException('Unsupported wrapping algorithm','NotSupportedError');
      const m = material(wrappingKey,alg.name,'wrapKey'), bytes = exportBytes(format,key);
      if (alg.name === 'AES-KW') return bufferOf(runOp(() => __obscuraCore.ops.op_subtle_aes_kw(true,m.bytes,bytes)));
      return cipher(true,alg,wrappingKey,bytes,'wrapKey');
    },
    async unwrapKey(format,wrappedKey,unwrappingKey,unwrapAlgorithm,unwrappedKeyAlgorithm,extractable,keyUsages) {
      format = keyFormat(format);
      const alg = normalizeAlgo(unwrapAlgorithm);
      if (!wrappingAlgorithms.has(alg.name)) throw new DOMException('Unsupported wrapping algorithm','NotSupportedError');
      const m = material(unwrappingKey,alg.name,'unwrapKey');
      const bytes = alg.name === 'AES-KW' ? runOp(() => __obscuraCore.ops.op_subtle_aes_kw(false,m.bytes,toBytes(wrappedKey))) : new Uint8Array(cipher(false,alg,unwrappingKey,wrappedKey,'unwrapKey'));
      let keyData = bytes;
      if (format === 'jwk') {
        try { keyData = __obscuraCore.ops.op_subtle_jwk_parse(bytes); }
        catch (_) { throw new DOMException('The unwrapped key is not a JsonWebKey','DataError'); }
      }
      return subtle.importKey(format,keyData,unwrappedKeyAlgorithm,extractable,keyUsages);
    },
  };
  globalThis.CryptoKey = CryptoKey;
  globalThis.SubtleCrypto = function SubtleCrypto() { throw new TypeError('Illegal constructor'); };
  Object.setPrototypeOf(subtle,globalThis.SubtleCrypto.prototype);
  globalThis.crypto.subtle = subtle;
  // Native Worker serializers retain this result privately in the realm owner.
  // Page snapshot initialization discards it, so authors cannot get key handles.
  return {wrapNativeKey:nativeKey};
}
})();