//! Kokoro-FastAPI (TTS) host install (moved from `tama_core::installations::tts_kokoro`
//! in plan-191 Task 10). Shared constants stay in `tama_core::installations::tts_kokoro::paths`.

pub mod download;

pub use download::install_kokoro_fastapi;
