// Public-key WebCrypto. Private material and immutable policy stay in native,
// garbage-collected handles; JavaScript receives only public metadata.
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use deno_core::{op2, v8, OpDecl};
use deno_error::JsErrorBox;
use openssl::{
    bn::{BigNum, BigNumContext, BigNumRef},
    derive::Deriver,
    ec::{EcGroup, EcKey, EcPoint, PointConversionForm},
    ecdsa::EcdsaSig,
    encrypt::{Decrypter, Encrypter},
    hash::MessageDigest,
    nid::Nid,
    pkey::{HasPublic, Id, PKey, PKeyRef, Private, Public},
    rsa::{Padding, Rsa},
    sign::{RsaPssSaltlen, Signer, Verifier},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{sync::Arc, time::Duration};

const MAX_KEY_BYTES: usize = 64 * 1024;
const MAX_INPUT_BYTES: usize = 16 * 1024 * 1024;
const ALL_USAGES: &[&str] = &["encrypt", "decrypt", "sign", "verify", "deriveKey", "deriveBits", "wrapKey", "unwrapKey"];

type Result<T, E = JsErrorBox> = std::result::Result<T, E>;
fn error(name: &str, message: impl std::fmt::Display) -> JsErrorBox {
    JsErrorBox::generic(format!("{name}: {message}"))
}
fn operation(e: impl std::fmt::Display) -> JsErrorBox { error("OperationError", e) }
fn data_error(e: impl std::fmt::Display) -> JsErrorBox { error("DataError", e) }
fn access() -> JsErrorBox { error("InvalidAccessError", "the key does not permit this operation") }

#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct Parameters {
    name: String,
    #[serde(default)] hash: String,
    #[serde(default)] named_curve: String,
    #[serde(default)] modulus_length: u32,
    #[serde(default)] public_exponent: Vec<u8>,
    #[serde(default)] salt_length: Option<u32>,
    #[serde(default)] length: Option<u32>,
}

enum Material { Private(PKey<Private>), Public(PKey<Public>), Secret(Vec<u8>) }
pub(crate) struct KeyData {
    material: Material,
    public: Option<PKey<Public>>,
    parameters: Parameters,
    extractable: bool,
    usages: Vec<String>,
    secret_algorithm: Option<Value>,
}
struct NativeKey(Arc<KeyData>);
unsafe impl deno_core::cppgc::GarbageCollected for NativeKey {
    fn trace(&self, _: &mut v8::cppgc::Visitor) {}
    fn get_name(&self) -> &'static std::ffi::CStr { c"WebCryptoKey" }
}

/// The only cross-isolate transport is an immutable native Arc. No V8 handle
/// or private DER is sent to a Worker thread or exposed to author JavaScript.
pub(crate) fn clone_native_key(scope: &mut v8::PinScope, value: v8::Local<v8::Value>) -> Option<Arc<KeyData>> {
    deno_core::cppgc::try_unwrap_cppgc_object::<NativeKey>(scope, value).map(|key| key.0.clone())
}
pub(crate) fn wrap_native_key<'s, 'i>(scope: &mut v8::PinScope<'s, 'i>, key: Arc<KeyData>) -> v8::Local<'s, v8::Object> {
    deno_core::cppgc::make_cppgc_object(scope, NativeKey(key))
}

#[op2]
#[cppgc]
fn op_public_key_clone(#[cppgc] key: &NativeKey) -> NativeKey { NativeKey(key.0.clone()) }

fn digest(name: &str) -> Result<MessageDigest> {
    match name {
        "SHA-1" => Ok(MessageDigest::sha1()),
        "SHA-256" => Ok(MessageDigest::sha256()),
        "SHA-384" => Ok(MessageDigest::sha384()),
        "SHA-512" => Ok(MessageDigest::sha512()),
        _ => Err(error("NotSupportedError", "unsupported hash")),
    }
}
fn curve(name: &str) -> Result<(Nid, usize)> {
    match name {
        "P-256" => Ok((Nid::X9_62_PRIME256V1, 32)),
        "P-384" => Ok((Nid::SECP384R1, 48)),
        "P-521" => Ok((Nid::SECP521R1, 66)),
        _ => Err(error("NotSupportedError", "unsupported named curve")),
    }
}
fn validate_algorithm(p: &Parameters) -> Result<()> {
    match p.name.as_str() {
        "RSA-OAEP" | "RSASSA-PKCS1-v1_5" | "RSA-PSS" => { digest(&p.hash)?; }
        "ECDSA" | "ECDH" => { curve(&p.named_curve)?; }
        "Ed25519" | "X25519" => {}
        _ => return Err(error("NotSupportedError", "unsupported public-key algorithm")),
    }
    Ok(())
}
fn allowed(name: &str, private: bool) -> &'static [&'static str] {
    match (name, private) {
        ("RSA-OAEP", false) => &["encrypt", "wrapKey"],
        ("RSA-OAEP", true) => &["decrypt", "unwrapKey"],
        ("ECDH" | "X25519", true) => &["deriveBits", "deriveKey"],
        ("ECDH" | "X25519", false) => &[],
        (_, true) => &["sign"],
        (_, false) => &["verify"],
    }
}
fn usages(name: &str, private: bool, requested: &[String]) -> Result<()> {
    if requested.iter().any(|u| !allowed(name, private).contains(&u.as_str())) {
        return Err(error("SyntaxError", "unsupported key usage"));
    }
    if private && requested.is_empty() {
        return Err(error("SyntaxError", "private keys require a usage"));
    }
    Ok(())
}
fn authorize(key: &KeyData, name: &str, usage: &str, private: bool) -> Result<()> {
    if key.parameters.name != name || !key.usages.iter().any(|u| u == usage)
        || matches!(key.material, Material::Private(_)) != private { return Err(access()); }
    Ok(())
}
fn private(key: &KeyData) -> Result<&PKeyRef<Private>> {
    match &key.material { Material::Private(k) => Ok(k), _ => Err(access()) }
}
fn public_der(key: &KeyData) -> Result<Vec<u8>> {
    key.public.as_ref().ok_or_else(access)?.public_key_to_der().map_err(operation)
}
fn public(key: &KeyData) -> Result<PKey<Public>> {
    key.public.clone().ok_or_else(access)
}
fn key_data(material: Material, p: Parameters, extractable: bool, requested: Vec<String>) -> Result<NativeKey> {
    let is_private = matches!(material, Material::Private(_));
    usages(&p.name, is_private, &requested)?;
    let key = public_from_material(&material)?;
    match p.name.as_str() {
        "RSA-OAEP" | "RSASSA-PKCS1-v1_5" | "RSA-PSS" => {
            if key.id() != Id::RSA { return Err(data_error("expected an RSA key")); }
            let rsa = key.rsa().map_err(data_error)?;
            if rsa.size() > 2048 || rsa.n().num_bits() < 256 || rsa.n().is_negative() || !rsa.n().is_odd() || rsa.e().is_negative() || rsa.e().num_bits() < 2 || !rsa.e().is_odd() || rsa.e().ucmp(rsa.n()) != std::cmp::Ordering::Less {
                return Err(data_error("invalid or oversized RSA key"));
            }
            if let Material::Private(k) = &material {
                let rsa = k.rsa().map_err(data_error)?;
                if [Some(rsa.d()), rsa.p(), rsa.q(), rsa.dmp1(), rsa.dmq1(), rsa.iqmp()].iter().flatten().any(|n| n.num_bytes() > 2048) {
                    return Err(data_error("oversized RSA private component"));
                }
                if !rsa.check_key().map_err(data_error)? { return Err(data_error("invalid RSA private key")); }
            }
        }
        "ECDSA" | "ECDH" => {
            if key.id() != Id::EC { return Err(data_error("expected an EC key")); }
            let ec = key.ec_key().map_err(data_error)?;
            if ec.group().curve_name() != Some(curve(&p.named_curve)?.0) { return Err(data_error("named curve mismatch")); }
            ec.check_key().map_err(data_error)?;
            if let Material::Private(k) = &material { k.ec_key().map_err(data_error)?.check_key().map_err(data_error)?; }
        }
        "Ed25519" | "X25519" => {
            let id = if p.name == "Ed25519" { Id::ED25519 } else { Id::X25519 };
            if key.id() != id { return Err(data_error("key type mismatch")); }
        }
        _ => return Err(error("NotSupportedError", "unsupported key algorithm")),
    }
    let normalized = ALL_USAGES.iter().filter(|usage| requested.iter().any(|u| u == **usage)).map(|usage| (*usage).to_owned()).collect();
    Ok(NativeKey(Arc::new(KeyData { material, public: Some(key), parameters: p, extractable, usages: normalized, secret_algorithm: None })))
}
fn public_from_material(material: &Material) -> Result<PKey<Public>> {
    match material {
        Material::Public(k) => Ok(k.clone()),
        Material::Private(k) => PKey::public_key_from_der(&k.public_key_to_der().map_err(data_error)?).map_err(data_error),
        Material::Secret(_) => Err(access()),
    }
}

#[op2]
#[cppgc]
async fn op_public_key_generate(#[serde] p: Parameters, extractable: bool, #[serde] requested: Vec<String>) -> Result<NativeKey> {
    validate_algorithm(&p)?;
    if requested.iter().any(|u| !allowed(&p.name, true).contains(&u.as_str()) && !allowed(&p.name, false).contains(&u.as_str())) {
        return Err(error("SyntaxError", "unsupported key usage"));
    }
    let private_usages: Vec<_> = requested.into_iter().filter(|u| allowed(&p.name, true).contains(&u.as_str())).collect();
    usages(&p.name, true, &private_usages)?;
    // Bound exponent work and total key size before native allocation/keygen.
    if p.name.starts_with("RSA") || p.name == "RSASSA-PKCS1-v1_5" {
        let exponent = p.public_exponent.iter().skip_while(|b| **b == 0).copied().collect::<Vec<_>>();
        if p.modulus_length < 256 || p.modulus_length > 16384 || exponent.is_empty() || exponent.len() > 4 {
            return Err(operation("invalid RSA modulus or exponent"));
        }
        let value = exponent.iter().fold(0u64, |a, b| (a << 8) | u64::from(*b));
        if value < 3 || value & 1 == 0 { return Err(operation("invalid RSA exponent")); }
    }
    // Native keygen never runs inside a V8 FFI callback. A bounded worker permit
    // remains held even if the JS operation times out or its document is retired.
    static KEYGEN: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(2);
    let permit = tokio::time::timeout(Duration::from_secs(15), KEYGEN.acquire()).await
        .map_err(|_| operation("native key generation admission deadline exceeded"))?
        .map_err(operation)?;
    let work = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let k = match p.name.as_str() {
            "RSA-OAEP" | "RSASSA-PKCS1-v1_5" | "RSA-PSS" => {
                let e = BigNum::from_slice(&p.public_exponent).map_err(operation)?;
                PKey::from_rsa(Rsa::generate_with_e(p.modulus_length, &e).map_err(operation)?).map_err(operation)?
            }
            "ECDSA" | "ECDH" => {
                let group = EcGroup::from_curve_name(curve(&p.named_curve)?.0).map_err(operation)?;
                PKey::from_ec_key(EcKey::generate(&group).map_err(operation)?).map_err(operation)?
            }
            "Ed25519" => PKey::generate_ed25519().map_err(operation)?,
            "X25519" => PKey::generate_x25519().map_err(operation)?,
            _ => return Err(error("NotSupportedError", "unsupported key algorithm")),
        };
        key_data(Material::Private(k), p, extractable, private_usages)
    });
    tokio::time::timeout(Duration::from_secs(15), work).await
        .map_err(|_| operation("native key generation deadline exceeded"))?
        .map_err(operation)?
}

#[op2]
#[cppgc]
fn op_public_key_public(#[cppgc] key: &NativeKey, #[serde] requested: Vec<String>) -> Result<NativeKey> {
    let requested = requested.into_iter().filter(|u| allowed(&key.0.parameters.name, false).contains(&u.as_str())).collect();
    key_data(Material::Public(public(&key.0)?), key.0.parameters.clone(), true, requested)
}

fn decode(j: &Value, name: &str) -> Result<Vec<u8>> {
    let text = j.get(name).and_then(Value::as_str).ok_or_else(|| data_error(format!("missing JWK {name}")))?;
    if text.len() > MAX_KEY_BYTES * 2 { return Err(data_error("oversized JWK member")); }
    URL_SAFE_NO_PAD.decode(text).map_err(data_error)
}
fn number(j: &Value, name: &str) -> Result<BigNum> {
    let bytes = decode(j, name)?;
    if bytes.is_empty() || bytes[0] == 0 { return Err(data_error("invalid base64url integer")); }
    BigNum::from_slice(&bytes).map_err(data_error)
}
fn jwk_algorithm(p: &Parameters) -> Option<String> {
    let suffix = p.hash.strip_prefix("SHA-").unwrap_or("");
    match p.name.as_str() {
        "RSA-OAEP" => Some(if p.hash == "SHA-1" { "RSA-OAEP".into() } else { format!("RSA-OAEP-{suffix}") }),
        "RSASSA-PKCS1-v1_5" => Some(format!("RS{suffix}")),
        "RSA-PSS" => Some(format!("PS{suffix}")),
        "ECDSA" => Some(match p.named_curve.as_str() { "P-256" => "ES256", "P-384" => "ES384", _ => "ES512" }.into()),
        "Ed25519" => Some("Ed25519".into()),
        _ => None,
    }
}
fn jwk_policy(j: &Value, p: &Parameters, extractable: bool, requested: &[String]) -> Result<()> {
    if !j.is_object() { return Err(data_error("JWK must be an object")); }
    if let Some(ext) = j.get("ext") {
        if !ext.is_boolean() || (ext == false && extractable) { return Err(data_error("JWK extractability mismatch")); }
    }
    if let Some(ops) = j.get("key_ops") {
        let list = ops.as_array().ok_or_else(|| data_error("invalid JWK key_ops"))?;
        let mut seen = std::collections::HashSet::new();
        for op in list {
            let op = op.as_str().ok_or_else(|| data_error("invalid JWK key_ops"))?;
            if !seen.insert(op) || !ALL_USAGES.contains(&op) { return Err(data_error("invalid JWK key_ops")); }
        }
        if requested.iter().any(|u| !seen.contains(u.as_str())) { return Err(data_error("JWK key_ops mismatch")); }
    }
    if let Some(use_) = j.get("use") {
        let expected = if matches!(p.name.as_str(), "RSA-OAEP" | "ECDH" | "X25519") { "enc" } else { "sig" };
        if !requested.is_empty() && use_.as_str() != Some(expected) { return Err(data_error("JWK use mismatch")); }
    }
    if let Some(alg) = j.get("alg") {
        if let Some(expected) = jwk_algorithm(p) {
            let legacy_ed = p.name == "Ed25519" && alg.as_str() == Some("EdDSA");
            if !legacy_ed && alg.as_str() != Some(expected.as_str()) { return Err(data_error("JWK alg mismatch")); }
        }
        // ECDH/X25519 do not bind JWK.alg; WebCrypto imports the curve/key
        // material independently of JOSE key-management negotiation.
    }
    Ok(())
}
fn from_jwk(j: &Value, p: &Parameters) -> Result<Material> {
    let private = j.get("d").is_some();
    match p.name.as_str() {
        "RSA-OAEP" | "RSASSA-PKCS1-v1_5" | "RSA-PSS" => {
            if j["kty"] != "RSA" || j.get("oth").is_some() { return Err(data_error("invalid RSA JWK")); }
            let n = number(j, "n")?; let e = number(j, "e")?;
            if private {
                let rsa = Rsa::from_private_components(n, e, number(j, "d")?, number(j, "p")?, number(j, "q")?, number(j, "dp")?, number(j, "dq")?, number(j, "qi")?).map_err(data_error)?;
                Ok(Material::Private(PKey::from_rsa(rsa).map_err(data_error)?))
            } else { Ok(Material::Public(PKey::from_rsa(Rsa::from_public_components(n, e).map_err(data_error)?).map_err(data_error)?)) }
        }
        "ECDSA" | "ECDH" => {
            if j["kty"] != "EC" || j["crv"] != p.named_curve { return Err(data_error("invalid EC JWK curve")); }
            let (nid, width) = curve(&p.named_curve)?;
            let x = decode(j, "x")?; let y = decode(j, "y")?;
            if x.len() != width || y.len() != width { return Err(data_error("invalid EC coordinate length")); }
            let group = EcGroup::from_curve_name(nid).map_err(data_error)?;
            let mut context = BigNumContext::new().map_err(data_error)?;
            let mut point = EcPoint::new(&group).map_err(data_error)?;
            let x = BigNum::from_slice(&x).map_err(data_error)?;
            let y = BigNum::from_slice(&y).map_err(data_error)?;
            point.set_affine_coordinates_gfp(&group, &x, &y, &mut context).map_err(data_error)?;
            if private {
                let d = decode(j, "d")?;
                if d.len() != width { return Err(data_error("invalid EC private scalar length")); }
                let d = BigNum::from_slice(&d).map_err(data_error)?;
                let ec = EcKey::from_private_components(&group, &d, &point).map_err(data_error)?;
                Ok(Material::Private(PKey::from_ec_key(ec).map_err(data_error)?))
            } else { Ok(Material::Public(PKey::from_ec_key(EcKey::from_public_key(&group, &point).map_err(data_error)?).map_err(data_error)?)) }
        }
        "Ed25519" | "X25519" => {
            if j["kty"] != "OKP" || j["crv"] != p.name { return Err(data_error("invalid OKP JWK")); }
            let id = if p.name == "Ed25519" { Id::ED25519 } else { Id::X25519 };
            let x = decode(j, "x")?;
            if x.len() != 32 { return Err(data_error("invalid OKP public key length")); }
            if private {
                let d = decode(j, "d")?;
                if d.len() != 32 { return Err(data_error("invalid OKP private key length")); }
                let k = PKey::private_key_from_raw_bytes(&d, id).map_err(data_error)?;
                if k.raw_public_key().map_err(data_error)? != x { return Err(data_error("OKP public/private mismatch")); }
                Ok(Material::Private(k))
            } else { Ok(Material::Public(PKey::public_key_from_raw_bytes(&x, id).map_err(data_error)?)) }
        }
        _ => Err(error("NotSupportedError", "unsupported JWK algorithm")),
    }
}
// Bounded DER validation precedes OpenSSL's permissive decoder. It rejects
// indefinite/nonminimal encodings and enforces each WebCrypto key container OID.
fn der_element(bytes: &[u8]) -> Result<(u8, &[u8], &[u8])> {
    if bytes.len() < 2 || bytes[0] & 31 == 31 { return Err(data_error("invalid DER tag")); }
    let (header, length) = if bytes[1] < 128 { (2, usize::from(bytes[1])) } else {
        let count = usize::from(bytes[1] & 127);
        if count == 0 || count > 4 || bytes.len() < 2 + count || bytes[2] == 0 { return Err(data_error("invalid DER length")); }
        let length = bytes[2..2 + count].iter().fold(0usize, |n, b| (n << 8) | usize::from(*b));
        if length < 128 { return Err(data_error("nonminimal DER length")); }
        (2 + count, length)
    };
    if length > bytes.len().saturating_sub(header) { return Err(data_error("truncated DER")); }
    Ok((bytes[0], &bytes[header..header + length], &bytes[header + length..]))
}
fn der_tree(mut bytes: &[u8], depth: usize) -> Result<()> {
    if depth > 16 { return Err(data_error("DER nesting limit exceeded")); }
    while !bytes.is_empty() {
        let (tag, content, rest) = der_element(bytes)?;
        if tag & 32 != 0 { der_tree(content, depth + 1)?; }
        if tag == 2 && (content.is_empty() || content.len() > 1 && (content[0] == 0 && content[1] < 128 || content[0] == 255 && content[1] >= 128)) {
            return Err(data_error("nonminimal DER integer"));
        }
        if tag == 3 && (content.is_empty() || content[0] > 7 || content.len() == 1 && content[0] != 0) { return Err(data_error("invalid DER bit string")); }
        if tag == 5 && !content.is_empty() { return Err(data_error("invalid DER NULL")); }
        bytes = rest;
    }
    Ok(())
}
fn exact_der(bytes: &[u8], format: &str, p: &Parameters) -> Result<()> {
    der_tree(bytes, 0)?;
    let (tag, mut content, rest) = der_element(bytes)?;
    if tag != 0x30 || !rest.is_empty() { return Err(data_error("expected one DER sequence")); }
    if format == "pkcs8" {
        let (tag, version, tail) = der_element(content)?;
        if tag != 2 || version != [0] { return Err(data_error("expected PKCS8 version zero")); }
        content = tail;
    }
    let (tag, algorithm, _) = der_element(content)?;
    if tag != 0x30 { return Err(data_error("expected an AlgorithmIdentifier")); }
    let (tag, oid, parameters) = der_element(algorithm)?;
    let expected: &[u8] = match p.name.as_str() {
        "RSA-OAEP" | "RSA-PSS" | "RSASSA-PKCS1-v1_5" => &[0x2a,0x86,0x48,0x86,0xf7,0x0d,1,1,1],
        "ECDSA" | "ECDH" => &[0x2a,0x86,0x48,0xce,0x3d,2,1],
        "Ed25519" => &[0x2b,0x65,0x70],
        "X25519" => &[0x2b,0x65,0x6e],
        _ => return Err(data_error("unsupported key OID")),
    };
    if tag != 6 || oid != expected { return Err(data_error("key algorithm OID mismatch")); }
    if matches!(p.name.as_str(), "Ed25519" | "X25519") && !parameters.is_empty() { return Err(data_error("OKP algorithm parameters must be absent")); }
    if p.name.starts_with("RSA") && parameters != [5,0] && !parameters.is_empty() { return Err(data_error("invalid RSA algorithm parameters")); }
    if matches!(p.name.as_str(), "ECDSA" | "ECDH") {
        let (tag, oid, rest) = der_element(parameters)?;
        let expected: &[u8] = match p.named_curve.as_str() { "P-256" => &[0x2a,0x86,0x48,0xce,0x3d,3,1,7], "P-384" => &[0x2b,0x81,4,0,0x22], _ => &[0x2b,0x81,4,0,0x23] };
        if tag != 6 || oid != expected || !rest.is_empty() { return Err(data_error("EC named curve OID mismatch")); }
    }
    Ok(())
}

#[op2]
#[cppgc]
fn op_public_key_import(#[string] format: &str, #[buffer] bytes: &[u8], #[serde] jwk: serde_json::Value, #[serde] p: Parameters, extractable: bool, #[serde] requested: Vec<String>) -> Result<NativeKey> {
    validate_algorithm(&p)?;
    match format {
        "spki" | "raw" => usages(&p.name, false, &requested)?,
        "pkcs8" => usages(&p.name, true, &requested)?,
        "jwk" => usages(&p.name, jwk.get("d").is_some(), &requested)?,
        _ => return Err(error("NotSupportedError", "unsupported key format")),
    }
    if bytes.len() > MAX_KEY_BYTES { return Err(data_error("key exceeds the native import limit")); }
    let material = match format {
        "jwk" => { jwk_policy(&jwk, &p, extractable, &requested)?; from_jwk(&jwk, &p)? }
        "spki" => { exact_der(bytes, format, &p)?; Material::Public(PKey::public_key_from_der(bytes).map_err(data_error)?) }
        "pkcs8" => { exact_der(bytes, format, &p)?; Material::Private(PKey::private_key_from_pkcs8(bytes).map_err(data_error)?) }
        "raw" => match p.name.as_str() {
            "ECDSA" | "ECDH" => {
                let (nid, width) = curve(&p.named_curve)?;
                if bytes.len() != 1 + width * 2 || bytes.first() != Some(&4) { return Err(data_error("expected an uncompressed EC public point")); }
                let group = EcGroup::from_curve_name(nid).map_err(data_error)?;
                let mut context = BigNumContext::new().map_err(data_error)?;
                let point = EcPoint::from_bytes(&group, bytes, &mut context).map_err(data_error)?;
                Material::Public(PKey::from_ec_key(EcKey::from_public_key(&group, &point).map_err(data_error)?).map_err(data_error)?)
            }
            "Ed25519" | "X25519" => {
                if bytes.len() != 32 { return Err(data_error("invalid raw public key length")); }
                Material::Public(PKey::public_key_from_raw_bytes(bytes, if p.name == "Ed25519" { Id::ED25519 } else { Id::X25519 }).map_err(data_error)?)
            }
            _ => return Err(error("NotSupportedError", "raw format is not supported for this algorithm")),
        },
        _ => return Err(error("NotSupportedError", "unsupported key format")),
    };
    key_data(material, p, extractable, requested)
}

#[op2]
#[serde]
fn op_public_key_metadata(#[cppgc] key: &NativeKey) -> Result<serde_json::Value> {
    if let Some(algorithm) = &key.0.secret_algorithm {
        return Ok(json!({"type":"secret", "algorithm":algorithm, "extractable":key.0.extractable, "usages":key.0.usages}));
    }
    let p = &key.0.parameters;
    let mut algorithm = json!({"name":p.name});
    if p.name.starts_with("RSA") {
        let rsa = public(&key.0)?.rsa().map_err(operation)?;
        algorithm["modulusLength"] = json!(rsa.n().num_bits());
        algorithm["publicExponent"] = json!(rsa.e().to_vec());
        algorithm["hash"] = json!({"name":p.hash});
    } else if matches!(p.name.as_str(), "ECDSA" | "ECDH") { algorithm["namedCurve"] = json!(p.named_curve); }
    Ok(json!({"type":if matches!(key.0.material, Material::Private(_)) { "private" } else { "public" }, "algorithm":algorithm, "extractable":key.0.extractable, "usages":key.0.usages}))
}
fn bn64(n: &BigNumRef) -> String { URL_SAFE_NO_PAD.encode(n.to_vec()) }
fn padded(n: &BigNumRef, width: usize) -> Result<Vec<u8>> { n.to_vec_padded(width as i32).map_err(operation) }
fn ec_jwk<T: HasPublic>(key: &PKeyRef<T>, p: &Parameters) -> Result<Value> {
    let ec = key.ec_key().map_err(operation)?;
    let mut context = BigNumContext::new().map_err(operation)?;
    let bytes = ec.public_key().to_bytes(ec.group(), PointConversionForm::UNCOMPRESSED, &mut context).map_err(operation)?;
    let width = curve(&p.named_curve)?.1;
    Ok(json!({"kty":"EC","crv":p.named_curve,"x":URL_SAFE_NO_PAD.encode(&bytes[1..1+width]),"y":URL_SAFE_NO_PAD.encode(&bytes[1+width..])}))
}
fn export_jwk(key: &KeyData) -> Result<Value> {
    let p = &key.parameters;
    let public = public(key)?;
    let mut j = match p.name.as_str() {
        "RSA-OAEP" | "RSA-PSS" | "RSASSA-PKCS1-v1_5" => {
            let rsa = public.rsa().map_err(operation)?;
            json!({"kty":"RSA","n":bn64(rsa.n()),"e":bn64(rsa.e())})
        }
        "ECDSA" | "ECDH" => ec_jwk(&public, p)?,
        _ => json!({"kty":"OKP","crv":p.name,"x":URL_SAFE_NO_PAD.encode(public.raw_public_key().map_err(operation)?)}),
    };
    if let Material::Private(k) = &key.material {
        match p.name.as_str() {
            "RSA-OAEP" | "RSA-PSS" | "RSASSA-PKCS1-v1_5" => {
                let rsa = k.rsa().map_err(operation)?;
                j["d"] = json!(bn64(rsa.d()));
                for (name, n) in [("p", rsa.p()), ("q", rsa.q()), ("dp", rsa.dmp1()), ("dq", rsa.dmq1()), ("qi", rsa.iqmp())] {
                    j[name] = json!(bn64(n.ok_or_else(|| operation("missing RSA private component"))?));
                }
            }
            "ECDSA" | "ECDH" => j["d"] = json!(URL_SAFE_NO_PAD.encode(padded(k.ec_key().map_err(operation)?.private_key(), curve(&p.named_curve)?.1)?)),
            _ => j["d"] = json!(URL_SAFE_NO_PAD.encode(k.raw_private_key().map_err(operation)?)),
        }
    }
    if p.name != "ECDSA" {
        if let Some(alg) = jwk_algorithm(p) { j["alg"] = json!(alg); }
    }
    j["ext"] = json!(key.extractable);
    j["key_ops"] = json!(key.usages);
    Ok(j)
}
#[op2]
#[buffer]
fn op_public_key_export(#[cppgc] key: &NativeKey, #[string] format: &str) -> Result<Vec<u8>> {
    if !key.0.extractable { return Err(access()); }
    match format {
        "jwk" => serde_json::to_vec(&export_jwk(&key.0)?).map_err(operation),
        "spki" if matches!(key.0.material, Material::Public(_)) => public_der(&key.0),
        "pkcs8" if matches!(key.0.material, Material::Private(_)) => private(&key.0)?.private_key_to_pkcs8().map_err(operation),
        "raw" if matches!(key.0.material, Material::Public(_)) => {
            let k = public(&key.0)?;
            if matches!(key.0.parameters.name.as_str(), "ECDSA" | "ECDH") {
                let ec = k.ec_key().map_err(operation)?;
                let mut context = BigNumContext::new().map_err(operation)?;
                ec.public_key().to_bytes(ec.group(), PointConversionForm::UNCOMPRESSED, &mut context).map_err(operation)
            } else if matches!(key.0.parameters.name.as_str(), "Ed25519" | "X25519") { k.raw_public_key().map_err(operation) }
            else { Err(error("NotSupportedError", "raw RSA keys are unsupported")) }
        }
        "spki" | "pkcs8" | "raw" => Err(error("InvalidAccessError", "format does not match key type")),
        _ => Err(error("NotSupportedError", "unsupported export format")),
    }
}

#[op2]
#[buffer]
fn op_public_key_sign(#[cppgc] key: &NativeKey, #[serde] p: Parameters, #[buffer] data: &[u8]) -> Result<Vec<u8>> {
    authorize(&key.0, &p.name, "sign", true)?;
    if data.len() > MAX_INPUT_BYTES { return Err(operation("message exceeds the native input limit")); }
    let k = private(&key.0)?;
    if p.name == "Ed25519" { return Signer::new_without_digest(k).map_err(operation)?.sign_oneshot_to_vec(data).map_err(operation); }
    let hash = if p.name == "ECDSA" { &p.hash } else { &key.0.parameters.hash };
    let md = digest(hash)?;
    let mut signer = Signer::new(md, k).map_err(operation)?;
    if p.name == "RSA-PSS" {
        let salt = salt_length(&key.0, &p)?;
        signer.set_rsa_padding(Padding::PKCS1_PSS).map_err(operation)?;
        signer.set_rsa_mgf1_md(md).map_err(operation)?;
        signer.set_rsa_pss_saltlen(RsaPssSaltlen::custom(salt)).map_err(operation)?;
    } else if p.name == "RSASSA-PKCS1-v1_5" { signer.set_rsa_padding(Padding::PKCS1).map_err(operation)?; }
    let signature = signer.sign_oneshot_to_vec(data).map_err(operation)?;
    if p.name != "ECDSA" { return Ok(signature); }
    let signature = EcdsaSig::from_der(&signature).map_err(operation)?;
    let width = curve(&key.0.parameters.named_curve)?.1;
    let mut raw = padded(signature.r(), width)?;
    raw.extend_from_slice(&padded(signature.s(), width)?);
    Ok(raw)
}
fn salt_length(key: &KeyData, p: &Parameters) -> Result<i32> {
    let salt = p.salt_length.ok_or_else(|| error("TypeError", "saltLength is required"))?;
    let bits = key.public.as_ref().ok_or_else(access)?.bits() as usize;
    let max = (bits - 1).div_ceil(8).saturating_sub(digest(&key.parameters.hash)?.size() + 2);
    if salt as usize > max || salt > i32::MAX as u32 { return Err(operation("invalid PSS salt length")); }
    Ok(salt as i32)
}
#[op2]
fn op_public_key_verify(#[cppgc] key: &NativeKey, #[serde] p: Parameters, #[buffer] signature: &[u8], #[buffer] data: &[u8]) -> Result<bool> {
    authorize(&key.0, &p.name, "verify", false)?;
    if data.len() > MAX_INPUT_BYTES || signature.len() > MAX_KEY_BYTES { return Err(operation("input exceeds the native limit")); }
    let k = public(&key.0)?;
    if p.name == "Ed25519" {
        return Ok(Verifier::new_without_digest(&k).map_err(operation)?.verify_oneshot(signature, data).unwrap_or(false));
    }
    let hash = if p.name == "ECDSA" { &p.hash } else { &key.0.parameters.hash };
    let md = digest(hash)?;
    let mut verifier = Verifier::new(md, &k).map_err(operation)?;
    if p.name == "RSA-PSS" {
        let salt = salt_length(&key.0, &p)?;
        verifier.set_rsa_padding(Padding::PKCS1_PSS).map_err(operation)?;
        verifier.set_rsa_mgf1_md(md).map_err(operation)?;
        verifier.set_rsa_pss_saltlen(RsaPssSaltlen::custom(salt)).map_err(operation)?;
    } else if p.name == "RSASSA-PKCS1-v1_5" { verifier.set_rsa_padding(Padding::PKCS1).map_err(operation)?; }
    let der;
    let signature = if p.name == "ECDSA" {
        let width = curve(&key.0.parameters.named_curve)?.1;
        if signature.len() != 2 * width { return Ok(false); }
        der = EcdsaSig::from_private_components(BigNum::from_slice(&signature[..width]).map_err(operation)?, BigNum::from_slice(&signature[width..]).map_err(operation)?).map_err(operation)?.to_der().map_err(operation)?;
        &der
    } else { signature };
    Ok(verifier.verify_oneshot(signature, data).unwrap_or(false))
}
#[op2]
#[buffer]
fn op_public_key_cipher(#[cppgc] key: &NativeKey, encrypt: bool, wrapping: bool, #[buffer] label: &[u8], #[buffer] data: &[u8]) -> Result<Vec<u8>> {
    authorize(&key.0, "RSA-OAEP", match (encrypt, wrapping) { (true,true) => "wrapKey", (false,true) => "unwrapKey", (true,false) => "encrypt", _ => "decrypt" }, !encrypt)?;
    if label.len() > MAX_INPUT_BYTES || data.len() > MAX_KEY_BYTES { return Err(operation("OAEP input exceeds the native limit")); }
    let md = digest(&key.0.parameters.hash)?;
    if encrypt {
        let k = public(&key.0)?;
        let mut cipher = Encrypter::new(&k).map_err(operation)?;
        cipher.set_rsa_padding(Padding::PKCS1_OAEP).map_err(operation)?;
        cipher.set_rsa_oaep_md(md).map_err(operation)?;
        cipher.set_rsa_mgf1_md(md).map_err(operation)?;
        if !label.is_empty() { cipher.set_rsa_oaep_label(label).map_err(operation)?; }
        let mut out = vec![0; cipher.encrypt_len(data).map_err(operation)?];
        let length = cipher.encrypt(data, &mut out).map_err(operation)?; out.truncate(length); Ok(out)
    } else {
        let mut cipher = Decrypter::new(private(&key.0)?).map_err(operation)?;
        cipher.set_rsa_padding(Padding::PKCS1_OAEP).map_err(operation)?;
        cipher.set_rsa_oaep_md(md).map_err(operation)?;
        cipher.set_rsa_mgf1_md(md).map_err(operation)?;
        if !label.is_empty() { cipher.set_rsa_oaep_label(label).map_err(operation)?; }
        let mut out = vec![0; cipher.decrypt_len(data).map_err(operation)?];
        let length = cipher.decrypt(data, &mut out).map_err(operation)?; out.truncate(length); Ok(out)
    }
}
#[op2]
#[buffer]
fn op_public_key_derive(#[cppgc] key: &NativeKey, #[cppgc] peer: &NativeKey, derive_key: bool, #[serde] p: Parameters) -> Result<Vec<u8>> {
    authorize(&key.0, &p.name, if derive_key { "deriveKey" } else { "deriveBits" }, true)?;
    if !matches!(p.name.as_str(), "ECDH" | "X25519") || peer.0.parameters.name != p.name
        || matches!(peer.0.material, Material::Private(_)) || peer.0.parameters.named_curve != key.0.parameters.named_curve { return Err(access()); }
    let public = public(&peer.0)?;
    let mut deriver = Deriver::new(private(&key.0)?).map_err(operation)?;
    deriver.set_peer(&public).map_err(operation)?;
    let mut secret = deriver.derive_to_vec().map_err(operation)?;
    if p.name == "X25519" && secret.iter().fold(0u8, |all, byte| all | byte) == 0 { return Err(operation("all-zero X25519 shared secret")); }
    if let Some(bits) = p.length {
        if bits as usize > secret.len() * 8 { return Err(operation("derived length exceeds the shared secret")); }
        secret.truncate((bits as usize).div_ceil(8));
        if bits % 8 != 0 {
            if let Some(last) = secret.last_mut() { *last &= 0xff << (8 - bits % 8); }
        }
    }
    Ok(secret)
}

// This payload is used only by the private IndexedDB platform-codec envelope.
// Native serialization never invokes author JSON/prototype hooks, and restore
// revalidates material and immutable policy. The record, not a runtime ticket
// table, owns its lifetime, so navigation/runtime replacement cannot invalidate it.
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct StoredKey {
    format: String,
    data: String,
    parameters: Parameters,
    extractable: bool,
    usages: Vec<String>,
    secret_algorithm: Option<Value>,
}
#[op2]
#[string]
fn op_public_key_serialize(#[cppgc] key: &NativeKey) -> Result<String> {
    let (format, data) = match &key.0.material {
        Material::Private(k) => ("pkcs8", URL_SAFE_NO_PAD.encode(k.private_key_to_pkcs8().map_err(operation)?)),
        Material::Public(k) => ("spki", URL_SAFE_NO_PAD.encode(k.public_key_to_der().map_err(operation)?)),
        Material::Secret(bytes) => ("secret", URL_SAFE_NO_PAD.encode(bytes)),
    };
    serde_json::to_string(&StoredKey { format:format.into(), data, parameters:key.0.parameters.clone(), extractable:key.0.extractable, usages:key.0.usages.clone(), secret_algorithm:key.0.secret_algorithm.clone() }).map_err(operation)
}
#[op2]
#[cppgc]
fn op_public_key_deserialize(#[string] payload: &str) -> Result<NativeKey> {
    if payload.len() > MAX_INPUT_BYTES * 2 { return Err(data_error("stored key exceeds the native input limit")); }
    let stored: StoredKey = serde_json::from_str(payload).map_err(data_error)?;
    let bytes = URL_SAFE_NO_PAD.decode(stored.data).map_err(data_error)?;
    if stored.format == "secret" {
        return secret_key_data(&bytes, stored.secret_algorithm.ok_or_else(|| data_error("missing secret algorithm"))?, stored.extractable, stored.usages);
    }
    if bytes.len() > MAX_KEY_BYTES || stored.secret_algorithm.is_some() { return Err(data_error("invalid stored key material")); }
    validate_algorithm(&stored.parameters)?;
    exact_der(&bytes, &stored.format, &stored.parameters)?;
    let material = match stored.format.as_str() {
        "spki" => Material::Public(PKey::public_key_from_der(&bytes).map_err(data_error)?),
        "pkcs8" => Material::Private(PKey::private_key_from_pkcs8(&bytes).map_err(data_error)?),
        _ => return Err(data_error("invalid stored key format")),
    };
    key_data(material, stored.parameters, stored.extractable, stored.usages)
}

// Secret primitives keep their existing byte-oriented RustCrypto ops. The
// private JS WeakMap may read bytes only through this private extension; Worker
// messages transport the native immutable Arc, never an author-owned byte field.
fn secret_key_data(bytes: &[u8], algorithm: Value, extractable: bool, requested: Vec<String>) -> Result<NativeKey> {
    if bytes.len() > MAX_INPUT_BYTES { return Err(data_error("secret key exceeds the native input limit")); }
    let name = algorithm["name"].as_str().ok_or_else(|| data_error("missing secret key algorithm"))?;
    let allowed: &[&str] = match name {
        "HMAC" => { digest(algorithm["hash"]["name"].as_str().unwrap_or(""))?; if bytes.is_empty() { return Err(data_error("empty HMAC key")); } &["sign","verify"] }
        "AES-GCM" | "AES-CBC" | "AES-CTR" | "AES-KW" => {
            if ![16,24,32].contains(&bytes.len()) { return Err(data_error("invalid AES key length")); }
            if name == "AES-KW" { &["wrapKey","unwrapKey"] } else { &["encrypt","decrypt","wrapKey","unwrapKey"] }
        }
        "PBKDF2" | "HKDF" => { if extractable { return Err(error("SyntaxError", "derivation keys cannot be extractable")); } &["deriveBits","deriveKey"] }
        _ => return Err(error("NotSupportedError", "unsupported secret-key algorithm")),
    };
    if requested.is_empty() || requested.iter().any(|usage| !allowed.contains(&usage.as_str())) { return Err(error("SyntaxError", "unsupported secret-key usage")); }
    let normalized = ALL_USAGES.iter().filter(|usage| requested.iter().any(|u| u == **usage)).map(|usage| (*usage).to_owned()).collect();
    Ok(NativeKey(Arc::new(KeyData { material: Material::Secret(bytes.to_vec()), public: None, parameters: Parameters {name:name.to_owned(), ..Parameters::default()}, extractable, usages: normalized, secret_algorithm: Some(algorithm) })))
}
#[op2]
#[cppgc]
fn op_secret_key_new(#[buffer] bytes: &[u8], #[serde] algorithm: serde_json::Value, extractable: bool, #[serde] requested: Vec<String>) -> Result<NativeKey> {
    secret_key_data(bytes, algorithm, extractable, requested)
}
#[op2]
#[buffer]
fn op_secret_key_bytes(#[cppgc] key: &NativeKey) -> Result<Vec<u8>> {
    match &key.0.material { Material::Secret(bytes) => Ok(bytes.clone()), _ => Err(access()) }
}

pub(crate) fn ops() -> Vec<OpDecl> {
    vec![op_public_key_generate(), op_public_key_public(), op_public_key_import(), op_public_key_metadata(), op_public_key_export(), op_public_key_sign(), op_public_key_verify(), op_public_key_cipher(), op_public_key_derive(), op_public_key_serialize(), op_public_key_deserialize(), op_public_key_clone(), op_secret_key_new(), op_secret_key_bytes()]
}
