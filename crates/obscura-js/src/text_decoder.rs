use std::cell::RefCell;
use deno_core::{op2, v8, OpDecl};
use deno_error::JsErrorBox;
use encoding_rs::{CoderResult, Decoder, DecoderResult, Encoding};

pub(crate) struct TextDecoderState {
    encoding: &'static Encoding,
    decoder: RefCell<Decoder>,
    fatal: bool,
    ignore_bom: bool,
}

// Contains only Rust-owned encoding state, with no V8 references.
unsafe impl deno_core::cppgc::GarbageCollected for TextDecoderState {
    fn trace(&self, _: &mut v8::cppgc::Visitor) {}
    fn get_name(&self) -> &'static std::ffi::CStr { c"TextDecoderState" }
}

impl TextDecoderState {
    fn fresh(&self) -> Decoder {
        if self.ignore_bom { self.encoding.new_decoder_without_bom_handling() }
        else { self.encoding.new_decoder_with_bom_removal() }
    }
}

#[op2]
#[cppgc]
fn op_text_decoder_new(#[string] label: &str, fatal: bool, ignore_bom: bool) -> Result<TextDecoderState, JsErrorBox> {
    let encoding = Encoding::for_label(label.as_bytes())
        .filter(|encoding| *encoding != encoding_rs::REPLACEMENT)
        .ok_or_else(|| JsErrorBox::range_error("Invalid TextDecoder encoding"))?;
    Ok(TextDecoderState {
        encoding,
        decoder: RefCell::new(if ignore_bom { encoding.new_decoder_without_bom_handling() }
            else { encoding.new_decoder_with_bom_removal() }),
        fatal, ignore_bom,
    })
}

#[op2]
#[string]
fn op_text_decoder_decode(#[cppgc] state: &TextDecoderState, #[buffer] input: &[u8], stream: bool) -> Result<String, JsErrorBox> {
    let mut decoder = state.decoder.borrow_mut();
    let capacity = if state.fatal { decoder.max_utf8_buffer_length_without_replacement(input.len()) }
        else { decoder.max_utf8_buffer_length(input.len()) }
        .ok_or_else(|| JsErrorBox::range_error("Decoded text exceeds string capacity"))?;
    let mut output = String::with_capacity(capacity);
    let mut remaining = input;
    let result = loop {
        if state.fatal {
            let (result, read) = decoder.decode_to_string_without_replacement(remaining, &mut output, !stream);
            remaining = &remaining[read..];
            match result {
                DecoderResult::InputEmpty => break Ok(output),
                DecoderResult::Malformed(..) => break Err(JsErrorBox::type_error("The encoded data was not valid")),
                DecoderResult::OutputFull => output.reserve(decoder.max_utf8_buffer_length_without_replacement(remaining.len()).unwrap_or(4).max(4)),
            }
        } else {
            let (result, read, _) = decoder.decode_to_string(remaining, &mut output, !stream);
            remaining = &remaining[read..];
            match result {
                CoderResult::InputEmpty => break Ok(output),
                CoderResult::OutputFull => output.reserve(decoder.max_utf8_buffer_length(remaining.len()).unwrap_or(4).max(4)),
            }
        }
    };
    // Ending the stream retires its BOM/pending-byte state even on failure.
    // A fatal streaming error leaves the live decoder in its consumed state.
    if !stream { *decoder = state.fresh(); }
    result
}

pub(crate) fn ops() -> Vec<OpDecl> { vec![op_text_decoder_new(), op_text_decoder_decode()] }
