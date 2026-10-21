use crate::runtime::ObscuraJsRuntime;
use serde_json::{json, Value};

async fn probe(script: &str) -> Value {
    let mut rt = ObscuraJsRuntime::new();
    rt.set_dom(obscura_dom::parse_html("<!doctype html><body></body>"));
    rt.set_url("https://webcrypto.test/");
    rt.run_page_init();
    rt.execute_script("webcrypto-regression", &format!(r#"
        globalThis.cryptoResult = null;
        const s = crypto.subtle;
        const hex = buf => Array.from(new Uint8Array(buf), b => b.toString(16).padStart(2, '0')).join('');
        const fromHex = h => new Uint8Array(h.match(/../g).map(b => parseInt(b, 16)));
        const errorName = async fn => {{ try {{ await fn(); return null; }} catch (e) {{ return e.name; }} }};
        (async () => {{ {script} }})().then(
            value => cryptoResult = value,
            error => cryptoResult = {{error:error.name, message:error.message}}
        );
    "#)).unwrap();
    rt.run_event_loop_bounded(20_000).await.unwrap();
    rt.evaluate("cryptoResult").unwrap()
}

#[tokio::test]
async fn aes_kw_wraps_and_unwraps_raw_keys() {
    // RFC 3394, section 4.1, through wrapKey / unwrapKey.
    let out = probe(r#"
        const kek = await s.importKey('raw', fromHex('000102030405060708090a0b0c0d0e0f'), 'AES-KW', false, ['wrapKey', 'unwrapKey']);
        const key = await s.importKey('raw', fromHex('00112233445566778899aabbccddeeff'), 'AES-CBC', true, ['encrypt']);
        const wrapped = await s.wrapKey('raw', key, kek, 'AES-KW');
        const back = await s.unwrapKey('raw', wrapped, kek, {name: 'AES-KW'}, 'AES-GCM', true, ['encrypt']);
        const tampered = new Uint8Array(wrapped.slice(0)); tampered[0] ^= 1;
        const generated = await s.generateKey({name: 'AES-KW', length: 256}, false, ['wrapKey', 'unwrapKey']);
        const roundTrip = await s.unwrapKey('raw', await s.wrapKey('raw', key, generated, 'AES-KW'), generated, 'AES-KW', 'AES-CBC', true, ['decrypt']);
        // RFC 3394 wraps at least two blocks, of 8 bytes each.
        const hmac8 = await s.importKey('raw', new Uint8Array(8).fill(1), {name: 'HMAC', hash: 'SHA-256'}, true, ['sign']);
        const hmac3 = await s.importKey('raw', new Uint8Array([1, 2, 3]), {name: 'HMAC', hash: 'SHA-256'}, true, ['sign', 'verify']);
        const hmac16 = await s.importKey('raw', new Uint8Array(16).fill(2), {name: 'HMAC', hash: 'SHA-256'}, true, ['sign']);
        const hmac16Back = await s.unwrapKey('raw', await s.wrapKey('raw', hmac16, kek, 'AES-KW'), kek, 'AES-KW', {name: 'HMAC', hash: 'SHA-256'}, true, ['sign']);
        return [
            hex(wrapped),
            back.algorithm.name, hex(await s.exportKey('raw', back)),
            hex(await s.exportKey('raw', roundTrip)),
            await errorName(() => s.unwrapKey('raw', tampered, kek, 'AES-KW', 'AES-GCM', true, ['encrypt'])),
            await errorName(() => s.encrypt('AES-KW', kek, new Uint8Array(16))),
            await errorName(() => s.wrapKey('raw', hmac8, kek, 'AES-KW')),
            // This JWK's JSON text is 77 bytes, not a multiple of 8.
            await errorName(() => s.wrapKey('jwk', hmac3, kek, 'AES-KW')),
            await errorName(() => s.unwrapKey('raw', new Uint8Array(16), kek, 'AES-KW', 'AES-GCM', true, ['encrypt'])),
            hmac16Back.algorithm.length,
        ];
    "#).await;
    assert_eq!(out, json!([
        "1fa68b0a8112b447aef34bd8fb5a7b829d3e862371d2cfe5",
        "AES-GCM", "00112233445566778899aabbccddeeff",
        "00112233445566778899aabbccddeeff",
        "OperationError",
        "NotSupportedError",
        "OperationError",
        "OperationError",
        "OperationError",
        128,
    ]));
}

#[tokio::test]
async fn aes_kw_supports_192_and_256_bit_wrapping_keys() {
    // RFC 3394, sections 4.2 and 4.3, through the public SubtleCrypto API.
    let out = probe(r#"
        const key = await s.importKey('raw', fromHex('00112233445566778899aabbccddeeff'), 'AES-GCM', true, ['encrypt']);
        const results = [];
        for (const length of [24, 32]) {
            const kek = await s.importKey('raw', Uint8Array.from({length}, (_, i) => i), 'AES-KW', false, ['wrapKey', 'unwrapKey']);
            const wrapped = await s.wrapKey('raw', key, kek, 'AES-KW');
            const restored = await s.unwrapKey('raw', wrapped, kek, 'AES-KW', 'AES-GCM', true, ['encrypt']);
            results.push([kek.algorithm.length, hex(wrapped), hex(await s.exportKey('raw', restored))]);
        }
        return results;
    "#).await;
    assert_eq!(out, json!([
        [192, "96778b25ae6ca435f92b5b97c050aed2468ab8a17ad84e5d", "00112233445566778899aabbccddeeff"],
        [256, "64e8c3f9ce0f5ba263e9777905818a2a93c8191e7d6e8ae7", "00112233445566778899aabbccddeeff"],
    ]));
}

#[tokio::test]
async fn wrapped_jwks_ignore_the_pages_json() {
    let out = probe(r#"
        const gcm = {name: 'AES-GCM', iv: new Uint8Array(12)};
        const wrapper = await s.generateKey({name: 'AES-GCM', length: 128}, true, ['wrapKey', 'unwrapKey', 'decrypt', 'encrypt']);
        const hmac = await s.importKey('raw', new Uint8Array([1, 2, 3]), {name: 'HMAC', hash: 'SHA-256'}, true, ['sign', 'verify']);
        Object.prototype.toJSON = () => ({injected: true});
        Array.prototype.toJSON = () => 'injected';
        const realParse = JSON.parse;
        JSON.parse = () => ({kty: 'oct', k: 'AAAA'});
        let text, unwrapped, sig;
        try {
            const wrapped = await s.wrapKey('jwk', hmac, wrapper, gcm);
            text = new TextDecoder().decode(await s.decrypt(gcm, wrapper, wrapped));
            unwrapped = await s.unwrapKey('jwk', wrapped, wrapper, gcm, {name: 'HMAC', hash: 'SHA-256'}, true, ['sign']);
        } finally {
            delete Object.prototype.toJSON;
            delete Array.prototype.toJSON;
            JSON.parse = realParse;
        }
        const notJson = await s.encrypt(gcm, wrapper, new TextEncoder().encode('not json'));
        return [
            text,
            JSON.stringify(await s.exportKey('jwk', hmac)) === text,
            hex(await s.exportKey('raw', unwrapped)),
            await errorName(() => s.unwrapKey('jwk', notJson, wrapper, gcm, {name: 'HMAC', hash: 'SHA-256'}, true, ['sign'])),
        ];
    "#).await;
    assert_eq!(out, json!([
        r#"{"alg":"HS256","ext":true,"k":"AQID","key_ops":["sign","verify"],"kty":"oct"}"#,
        true,
        "010203",
        "DataError",
    ]));
}

#[tokio::test]
async fn unwrapped_jwks_accept_unknown_members_with_large_json_numbers() {
    let out = probe(r#"
        const gcm = {name: 'AES-GCM', iv: new Uint8Array(12)};
        const wrapper = await s.generateKey({name: 'AES-GCM', length: 128}, false, ['encrypt', 'unwrapKey']);
        const text = '{"kty":"oct","k":"AAAAAAAAAAAAAAAAAAAAAA","ext":true,"key_ops":["encrypt"],"vendor":1e400}';
        const wrapped = await s.encrypt(gcm, wrapper, new TextEncoder().encode(text));
        const key = await s.unwrapKey('jwk', wrapped, wrapper, gcm, 'AES-GCM', true, ['encrypt']);
        return hex(await s.exportKey('raw', key));
    "#).await;
    assert_eq!(out, json!("00000000000000000000000000000000"));
}

#[tokio::test]
async fn unwrapped_jwks_reject_non_object_json() {
    let out = probe(r#"
        const gcm = {name: 'AES-GCM', iv: new Uint8Array(12)};
        const wrapper = await s.generateKey({name: 'AES-GCM', length: 128}, false, ['encrypt', 'unwrapKey']);
        const results = [];
        for (const text of ['not json', '[1, 2]', '"oct"', 'null']) {
            const wrapped = await s.encrypt(gcm, wrapper, new TextEncoder().encode(text));
            results.push(await errorName(() => s.unwrapKey('jwk', wrapped, wrapper, gcm, 'AES-GCM', true, ['encrypt'])));
        }
        return results;
    "#).await;
    assert_eq!(out, json!(["DataError", "DataError", "DataError", "DataError"]));
}

#[tokio::test]
async fn key_wrapping_checks_the_wrapping_keys_algorithm_and_usage() {
    let out = probe(r#"
        const bytes = new Uint8Array(16);
        const payload = await s.importKey('raw', bytes, 'AES-GCM', true, ['encrypt']);
        const results = [];
        for (const name of ['AES-KW', 'AES-GCM', 'AES-CBC', 'AES-CTR']) {
            const alg = name === 'AES-GCM' ? {name, iv: new Uint8Array(12)}
                : name === 'AES-CBC' ? {name, iv: new Uint8Array(16)}
                : name === 'AES-CTR' ? {name, counter: new Uint8Array(16), length: 64} : name;
            const unwrapOnly = await s.importKey('raw', bytes, name, false, ['unwrapKey']);
            const wrongAlgorithm = await s.importKey('raw', bytes, {name:'HMAC', hash:'SHA-256'}, false, ['sign']);
            results.push(await errorName(() => s.wrapKey('raw', payload, unwrapOnly, alg)));
            results.push(await errorName(() => s.wrapKey('raw', payload, wrongAlgorithm, alg)));
            wrongAlgorithm.algorithm.name = name;
            results.push(await errorName(() => s.wrapKey('raw', payload, wrongAlgorithm, alg)));
            const wrapOnly = await s.importKey('raw', bytes, name, false, ['wrapKey']);
            wrapOnly.algorithm.name = 'HMAC';
            results.push((await s.wrapKey('raw', payload, wrapOnly, alg)).byteLength > 0);
        }
        return results;
    "#).await;
    assert_eq!(out, json!([
        "InvalidAccessError", "InvalidAccessError", "InvalidAccessError", true,
        "InvalidAccessError", "InvalidAccessError", "InvalidAccessError", true,
        "InvalidAccessError", "InvalidAccessError", "InvalidAccessError", true,
        "InvalidAccessError", "InvalidAccessError", "InvalidAccessError", true,
    ]));
}

#[tokio::test]
async fn key_unwrapping_checks_the_unwrapping_keys_algorithm_and_usage() {
    let out = probe(r#"
        const bytes = new Uint8Array(16);
        const payload = await s.importKey('raw', bytes, 'AES-GCM', true, ['encrypt']);
        const results = [];
        for (const name of ['AES-KW', 'AES-GCM', 'AES-CBC', 'AES-CTR']) {
            const alg = name === 'AES-GCM' ? {name, iv: new Uint8Array(12)}
                : name === 'AES-CBC' ? {name, iv: new Uint8Array(16)}
                : name === 'AES-CTR' ? {name, counter: new Uint8Array(16), length: 64} : name;
            const wrapOnly = await s.importKey('raw', bytes, name, false, ['wrapKey']);
            const wrapped = await s.wrapKey('raw', payload, wrapOnly, alg);
            const wrongAlgorithm = await s.importKey('raw', bytes, {name:'HMAC', hash:'SHA-256'}, false, ['sign']);
            const unwrap = key => s.unwrapKey('raw', wrapped, key, alg, 'AES-GCM', true, ['encrypt']);
            results.push(await errorName(() => unwrap(wrapOnly)));
            results.push(await errorName(() => unwrap(wrongAlgorithm)));
            wrongAlgorithm.algorithm.name = name;
            results.push(await errorName(() => unwrap(wrongAlgorithm)));
            const unwrapOnly = await s.importKey('raw', bytes, name, false, ['unwrapKey']);
            unwrapOnly.algorithm.name = 'HMAC';
            const restored = await unwrap(unwrapOnly);
            results.push(hex(await s.exportKey('raw', restored)));
        }
        return results;
    "#).await;
    assert_eq!(out, json!([
        "InvalidAccessError", "InvalidAccessError", "InvalidAccessError", "00000000000000000000000000000000",
        "InvalidAccessError", "InvalidAccessError", "InvalidAccessError", "00000000000000000000000000000000",
        "InvalidAccessError", "InvalidAccessError", "InvalidAccessError", "00000000000000000000000000000000",
        "InvalidAccessError", "InvalidAccessError", "InvalidAccessError", "00000000000000000000000000000000",
    ]));
}

#[tokio::test]
async fn unsupported_wrapping_algorithms_still_report_not_supported() {
    let out = probe(r#"
        const key = await s.importKey('raw', new Uint8Array(16), {name:'HMAC', hash:'SHA-256'}, true, ['sign']);
        return [
            await errorName(() => s.wrapKey('raw', key, key, 'HMAC')),
            await errorName(() => s.unwrapKey('raw', new Uint8Array(24), key, 'HMAC', 'AES-GCM', false, ['encrypt'])),
        ];
    "#).await;
    assert_eq!(out, json!(["NotSupportedError", "NotSupportedError"]));
}

#[tokio::test]
async fn cloned_wrapping_keys_keep_their_internal_algorithm() {
    let out = probe(r#"
        const key = await s.importKey('raw', new Uint8Array(16), 'AES-KW', false, ['wrapKey', 'unwrapKey']);
        key.algorithm.name = 'HMAC';
        const copy = structuredClone({first:key, second:key});
        const payload = await s.importKey('raw', new Uint8Array(16), 'AES-GCM', true, ['encrypt']);
        const wrapped = await s.wrapKey('raw', payload, copy.first, 'AES-KW');
        const restored = await s.unwrapKey('raw', wrapped, copy.first, 'AES-KW', 'AES-GCM', true, ['encrypt']);
        return [copy.first.algorithm.name, copy.first === copy.second, hex(await s.exportKey('raw', restored))];
    "#).await;
    assert_eq!(out, json!(["AES-KW", true, "00000000000000000000000000000000"]));
}

#[tokio::test]
async fn public_keys_match_ed25519_and_x25519_known_vectors() {
    let out = probe(r#"
        const b64 = h => btoa(String.fromCharCode(...fromHex(h))).replace(/\+/g,'-').replace(/\//g,'_').replace(/=+$/,'');
        const ed = await s.importKey('jwk', {kty:'OKP',crv:'Ed25519',x:b64('d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a'),d:b64('9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60')}, 'Ed25519', false, ['sign']);
        const signature = await s.sign('Ed25519', ed, new Uint8Array());
        const alice = await s.importKey('jwk', {kty:'OKP',crv:'X25519',d:b64('77076d0a7318a57d3c16c17251b26645df4c2f87ebc0992ab177fba51db92c2a'),x:b64('8520f0098930a754748b7ddcb43ef75a0dbf3a0d26381af4eba4a98eaa9b4e6a')}, 'X25519', false, ['deriveBits']);
        const bob = await s.importKey('raw', fromHex('de9edb7d7b7dc1b4d35b61c2ece435373f8343c85b78674dadfc7e146f882b4f'), 'X25519', true, []);
        const copy = structuredClone(ed);
        ed.algorithm.name = 'X25519';
        return [hex(signature), hex(await s.deriveBits({name:'X25519',public:bob},alice,256)), hex(await s.sign('Ed25519',copy,new Uint8Array())), await errorName(() => s.exportKey('pkcs8',copy)), await errorName(() => s.sign('Ed25519',bob,new Uint8Array())), Object.keys(copy).sort()];
    "#).await;
    let signature = "e5564300c360ac729086e2cc806e828a84877f1eb8e5d974d873e065224901555fb8821590a33bacc61e39701cf9b46bd25bf5f0595bbe24655141438e7a100b";
    assert_eq!(out, json!([signature, "4a5d9d5ba4ce2de1728e3bf480350f25e07e21c947d19e3376f09b3c1e161742", signature, "InvalidAccessError", "InvalidAccessError", ["algorithm","extractable","type","usages"]]));
}

#[tokio::test]
async fn public_crypto_roundtrips_all_families_and_formats() {
    let out = probe(r#"
        const message = new TextEncoder().encode('public WebCrypto consumer');
        const results = [];
        for (const name of ['RSA-OAEP','RSASSA-PKCS1-v1_5','RSA-PSS','ECDSA','ECDH','Ed25519','X25519']) {
            const rsa = name.startsWith('RSA'), agreement = name === 'ECDH' || name === 'X25519';
            const algorithm = rsa ? {name,modulusLength:1024,publicExponent:new Uint8Array([1,0,1]),hash:'SHA-256'} : name.startsWith('EC') ? {name,namedCurve:'P-256'} : {name};
            const uses = agreement ? ['deriveKey','deriveBits'] : name === 'RSA-OAEP' ? ['encrypt','decrypt','wrapKey','unwrapKey'] : ['sign','verify'];
            const pair = await s.generateKey(algorithm,true,uses);
            const formats = rsa ? ['spki','jwk'] : ['raw','spki','jwk'];
            let valid = true;
            for (const format of formats) {
                const publicKey = await s.importKey(format,await s.exportKey(format,pair.publicKey),algorithm,true,pair.publicKey.usages);
                for (const privateFormat of ['pkcs8','jwk']) {
                    const privateKey = await s.importKey(privateFormat,await s.exportKey(privateFormat,pair.privateKey),algorithm,true,pair.privateKey.usages);
                    if (agreement) {
                        const a = await s.deriveBits({name,public:publicKey},privateKey,128);
                        valid &&= hex(a) === hex(await s.deriveBits({name,public:pair.publicKey},pair.privateKey,128));
                    } else if (name === 'RSA-OAEP') {
                        const op = {name,label:new Uint8Array([7,8,9])};
                        valid &&= hex(await s.decrypt(op,privateKey,await s.encrypt(op,publicKey,message))) === hex(message);
                    } else {
                        const op = name === 'ECDSA' ? {name,hash:'SHA-256'} : name === 'RSA-PSS' ? {name,saltLength:0} : {name};
                        const sig = await s.sign(op,privateKey,message);
                        valid &&= await s.verify(op,publicKey,sig,message);
                        valid &&= !(await s.verify(op,publicKey,sig,new Uint8Array([1])));
                    }
                }
            }
            results.push([name,valid,pair.publicKey.extractable,pair.privateKey.type]);
        }
        return results;
    "#).await;
    assert_eq!(out, json!([ ["RSA-OAEP",true,true,"private"], ["RSASSA-PKCS1-v1_5",true,true,"private"], ["RSA-PSS",true,true,"private"], ["ECDSA",true,true,"private"], ["ECDH",true,true,"private"], ["Ed25519",true,true,"private"], ["X25519",true,true,"private"] ]));
}

#[tokio::test]
async fn public_key_wrapping_and_derivation_enforce_internal_capabilities() {
    let out = probe(r#"
        const a = await s.generateKey({name:'RSA-OAEP',modulusLength:1024,publicExponent:new Uint8Array([1,0,1]),hash:'SHA-256'},false,['wrapKey','unwrapKey']);
        const secret = await s.generateKey({name:'AES-GCM',length:128},true,['encrypt','decrypt']);
        const wrapped = await s.wrapKey('raw',secret,a.publicKey,{name:'RSA-OAEP',label:new Uint8Array([1])});
        const restored = await s.unwrapKey('raw',wrapped,a.privateKey,{name:'RSA-OAEP',label:new Uint8Array([1])},'AES-GCM',true,['encrypt','decrypt']);
        const ec = await s.generateKey({name:'ECDH',namedCurve:'P-521'},false,['deriveBits']);
        const tooLong = await errorName(() => s.deriveBits({name:'ECDH',public:ec.publicKey},ec.privateKey,529));
        const noUsage = await errorName(() => s.deriveKey({name:'ECDH',public:ec.publicKey},ec.privateKey,{name:'AES-GCM',length:128},false,['encrypt']));
        const wrongLabel = await errorName(() => s.unwrapKey('raw',wrapped,a.privateKey,'RSA-OAEP','AES-GCM',false,['encrypt']));
        const gcm = {name:'AES-GCM',iv:new Uint8Array(12)};
        const msg = new Uint8Array([1,2,3]);
        return [hex(await s.decrypt(gcm,restored,await s.encrypt(gcm,secret,msg))),tooLong,noUsage,wrongLabel,(await s.deriveBits({name:'ECDH',public:ec.publicKey},ec.privateKey,0)).byteLength,(await s.deriveBits({name:'ECDH',public:ec.publicKey},ec.privateKey)).byteLength,await errorName(() => s.importKey('jwk',{kty:'EC',crv:'P-256'}, {name:'ECDSA',namedCurve:'P-384'},true,['verify'])),await errorName(() => s.generateKey('Ed25519',true,['encrypt']))];
    "#).await;
    assert_eq!(out, json!(["010203","OperationError","InvalidAccessError","OperationError",0,66,"DataError","SyntaxError"]));
}

#[tokio::test]
async fn rsa_pkcs1_signature_matches_rfc7515_appendix_a2() {
    let out = probe(r#"
        const key = await s.importKey('jwk', {
            kty:'RSA',e:'AQAB',
            n:'ofgWCuLjybRlzo0tZWJjNiuSfb4p4fAkd_wWJcyQoTbji9k0l8W26mPddxHmfHQp-Vaw-4qPCJrcS2mJPMEzP1Pt0Bm4d4QlL-yRT-SFd2lZS-pCgNMsD1W_YpRPEwOWvG6b32690r2jZ47soMZo9wGzjb_7OMg0LOL-bSf63kpaSHSXndS5z5rexMdbBYUsLA9e-KXBdQOS-UTo7WTBEMa2R2CapHg665xsmtdVMTBQY4uDZlxvb3qCo5ZwKh9kG4LT6_I5IhlJH7aGhyxXFvUK-DWNmoudF8NAco9_h9iaGNj8q2ethFkMLs91kzk2PAcDTW9gb54h4FRWyuXpoQ',
            d:'Eq5xpGnNCivDflJsRQBXHx1hdR1k6Ulwe2JZD50LpXyWPEAeP88vLNO97IjlA7_GQ5sLKMgvfTeXZx9SE-7YwVol2NXOoAJe46sui395IW_GO-pWJ1O0BkTGoVEn2bKVRUCgu-GjBVaYLU6f3l9kJfFNS3E0QbVdxzubSu3Mkqzjkn439X0M_V51gfpRLI9JYanrC4D4qAdGcopV_0ZHHzQlBjudU2QvXt4ehNYTCBr6XCLQUShb1juUO1ZdiYoFaFQT5Tw8bGUl_x_jTj3ccPDVZFD9pIuhLhBOneufuBiB4cS98l2SR_RQyGWSeWjnczT0QU91p1DhOVRuOopznQ',
            p:'4BzEEOtIpmVdVEZNCqS7baC4crd0pqnRH_5IB3jw3bcxGn6QLvnEtfdUdiYrqBdss1l58BQ3KhooKeQTa9AB0Hw_Py5PJdTJNPY8cQn7ouZ2KKDcmnPGBY5t7yLc1QlQ5xHdwW1VhvKn-nXqhJTBgIPgtldC-KDV5z-y2XDwGUc',
            q:'uQPEfgmVtjL0Uyyx88GZFF1fOunH3-7cepKmtH4pxhtCoHqpWmT8YAmZxaewHgHAjLYsp1ZSe7zFYHj7C6ul7TjeLQeZD_YwD66t62wDmpe_HlB-TnBA-njbglfIsRLtXlnDzQkv5dTltRJ11BKBBypeeF6689rjcJIDEz9RWdc',
            dp:'BwKfV3Akq5_MFZDFZCnW-wzl-CCo83WoZvnLQwCTeDv8uzluRSnm71I3QCLdhrqE2e9YkxvuxdBfpT_PI7Yz-FOKnu1R6HsJeDCjn12Sk3vmAktV2zb34MCdy7cpdTh_YVr7tss2u6vneTwrA86rZtu5Mbr1C1XsmvkxHQAdYo0',
            dq:'h_96-mK1R_7glhsum81dZxjTnYynPbZpHziZjeeHcXYsXaaMwkOlODsWa7I9xXDoRwbKgB719rrmI2oKr6N3Do9U0ajaHF-NKJnwgjMd2w9cjz3_-kyNlxAr2v4IKhGNpmM5iIgOS1VZnOZ68m6_pbLBSp3nssTdlqvd0tIiTHU',
            qi:'IYd7DHOhrWvxkwPQsRM2tOgrjbcrfvtQJipd-DlcxyVuuM9sQLdgjVk2oy26F0EmpScGLq2MowX7fhd_QJQ3ydy5cY7YIBi87w93IKLEdfnbJtoOPLUW0ITrJReOgo1cq9SbsxYawBgfp_gh6A5603k2-ZQwVK0JKSHuLFkuQ3U'
        },{name:'RSASSA-PKCS1-v1_5',hash:'SHA-256'},false,['sign']);
        const input = 'eyJhbGciOiJSUzI1NiJ9.eyJpc3MiOiJqb2UiLA0KICJleHAiOjEzMDA4MTkzODAsDQogImh0dHA6Ly9leGFtcGxlLmNvbS9pc19yb290Ijp0cnVlfQ';
        const signature = new Uint8Array(await s.sign('RSASSA-PKCS1-v1_5',key,new TextEncoder().encode(input)));
        return btoa(String.fromCharCode(...signature)).replace(/\+/g,'-').replace(/\//g,'_').replace(/=+$/,'');
    "#).await;
    assert_eq!(out, json!("cC4hiUPoj9Eetdgtv3hF80EGrhuB__dzERat0XF9g2VtQgr9PJbu3XOiZj5RZmh7AAuHIm4Bh-0Qc_lF5YKt_O8W2Fp5jujGbds9uJdbF9CUAr7t1dnZcAcQjbKBYNX4BAynRFdiuB--f_nZLgrnbyTyWzO75vRK5h6xBArLIARNPvkSjtQBMHlb1L07Qe7K0GarZRmB_eSN9383LcOLn6_dO--xi12jzDwusC-eOkHWEsqtFZESc6BfI7noOPqvhJ1phCnvWh6IeYI2w9QOYEUipUTI8np6LbgGY9Fs98rqVt5AXLIhWkWywlVmtVrBp0igcN_IoypGlUPQGe77Rw"));
}
