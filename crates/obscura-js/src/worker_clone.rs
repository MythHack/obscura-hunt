use super::*;

pub(super) struct PlatformValue {
    kind: u32,
    bytes: Vec<u8>,
    mime: String,
    name: String,
    modified: f64,
}
impl PlatformValue {
    pub fn len(&self) -> usize { self.bytes.len() }
    pub fn read(scope: &mut v8::PinScope, callback: &v8::Global<v8::Function>, object: v8::Local<v8::Object>) -> Option<Self> {
        let callback = v8::Local::new(scope,callback); let receiver = v8::undefined(scope);
        let value = callback.call(scope,receiver.into(),&[object.into()])?.try_cast::<v8::Object>().ok()?;
        fn field<'s>(scope: &mut v8::PinScope<'s,'_>, value: v8::Local<v8::Object>, name: &str) -> Option<v8::Local<'s,v8::Value>> {
            let key = v8::String::new(scope,name)?; value.get(scope,key.into())
        }
        let bytes = field(scope,value,"bytes")?.try_cast::<v8::Uint8Array>().ok()?;
        if bytes.byte_length() > MAX_MESSAGE_BYTES { return None; }
        let mut output = vec![0;bytes.byte_length()]; bytes.copy_contents(&mut output);
        let kind = field(scope,value,"kind")?.uint32_value(scope)?;
        let mime = field(scope,value,"mime")?.to_rust_string_lossy(scope);
        let name = field(scope,value,"name")?.to_rust_string_lossy(scope);
        let modified = field(scope,value,"modified")?.number_value(scope)?;
        Some(Self { kind,bytes:output,mime,name,modified })
    }
    pub fn write<'s>(self, scope: &mut v8::PinScope<'s,'_>, callback: &v8::Global<v8::Function>) -> Option<v8::Local<'s,v8::Object>> {
        let length = self.bytes.len();
        let buffer = v8::ArrayBuffer::new(scope,length);
        let store = buffer.get_backing_store();
        if let Some(destination) = store.data() {
            // V8's sandbox requires recipient-owned backing memory.
            unsafe { std::ptr::copy_nonoverlapping(self.bytes.as_ptr(),destination.as_ptr().cast::<u8>(),length); }
        }
        let bytes = v8::Uint8Array::new(scope,buffer,0,length)?;
        let kind = v8::Integer::new_from_unsigned(scope,self.kind);
        let mime = v8::String::new(scope,&self.mime)?; let name = v8::String::new(scope,&self.name)?;
        let modified = v8::Number::new(scope,self.modified);
        let callback = v8::Local::new(scope,callback); let receiver = v8::undefined(scope);
        callback.call(scope,receiver.into(),&[kind.into(),bytes.into(),mime.into(),name.into(),modified.into()])?.try_cast().ok()
    }
}
