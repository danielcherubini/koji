"""LLMLingua-2 compaction server for Tama proxy."""

import os
import sys
import time
from fastapi import FastAPI
from pydantic import BaseModel, Field
from typing import Optional, List, Dict, Any, Literal, Union
from llmlingua import PromptCompressor
import warnings

warnings.filterwarnings("ignore", category=FutureWarning)

app = FastAPI(title="Tama Compaction Server")

# Model configuration
MODEL_NAME = os.environ.get(
    "COMPACTION_MODEL",
    "microsoft/llmlingua-2-xlm-roberta-large-meetingbank"
)
DEVICE = os.environ.get("COMPACTION_DEVICE", "cpu")

# Global compressor — loaded once at startup
_compressor: Optional[PromptCompressor] = None
_model_load_time: Optional[float] = None


def get_compressor() -> PromptCompressor:
    """Lazy-load the PromptCompressor on first call."""
    global _compressor, _model_load_time
    if _compressor is None:
        start = time.time()
        _compressor = PromptCompressor(
            model_name=MODEL_NAME,
            use_llmlingua2=True,
            device_map=DEVICE,
        )
        _model_load_time = time.time() - start
    return _compressor


class TextCompressRequest(BaseModel):
    """Request for raw text compression."""
    mode: Literal["text"] = "text"
    text: str
    rate: float = Field(default=0.3, ge=0.01, le=1.0)
    force_tokens: List[str] = Field(default_factory=lambda: ["\n"])
    chunk_end_tokens: List[str] = Field(default_factory=lambda: [".", "\n"])


class MessagesCompressRequest(BaseModel):
    """Request for OpenAI messages compression."""
    mode: Literal["messages"] = "messages"
    messages: List[Dict[str, Any]]
    rates: Dict[str, float] = Field(
        default_factory=lambda: {
            "system": 0.8,
            "user": 0.3,
            "assistant": 0.3,
            "default": 0.3,
        }
    )
    force_tokens: List[str] = Field(default_factory=lambda: ["\n"])
    chunk_end_tokens: List[str] = Field(default_factory=lambda: [".", "\n"])


class CompressResponse(BaseModel):
    """Response from compression."""
    compressed_text: Optional[str] = None
    compressed_messages: Optional[List[Dict[str, Any]]] = None
    original_tokens: int = 0
    compressed_tokens: int = 0
    compression_ratio: float = 1.0
    latency_ms: int = 0
    status: Literal["compressed", "skipped"] = "compressed"
    warmup: bool = False


@app.get("/health")
async def health_check():
    """Health check endpoint."""
    return {"status": "OK"}


@app.post("/compress")
async def compress(request: Union[TextCompressRequest, MessagesCompressRequest]):
    """Compress text or messages using LLMLingua-2."""
    start = time.time()
    compressor = get_compressor()
    warmup = _model_load_time is not None and (time.time() - _model_load_time) < 1.0

    if request.mode == "text":
        return _compress_text(
            compressor, request.text, request.rate,
            request.force_tokens, request.chunk_end_tokens, start, warmup
        )
    else:
        return _compress_messages(
            compressor, request.messages, request.rates,
            request.force_tokens, request.chunk_end_tokens, start, warmup
        )


def _compress_text(
    compressor, text: str, rate: float,
    force_tokens: List[str], chunk_end_tokens: List[str],
    start: float, warmup: bool
) -> CompressResponse:
    """Compress raw text."""
    try:
        result = compressor.compress_prompt_llmlingua2(
            text,
            rate=rate,
            force_tokens=force_tokens,
            chunk_end_tokens=chunk_end_tokens,
            return_word_label=False,
            drop_consecutive=True,
        )
        latency_ms = int((time.time() - start) * 1000)
        original = result.get("origin_tokens", 0)
        compressed = result.get("compressed_tokens", 0)
        ratio = original / compressed if compressed > 0 else 1.0
        return CompressResponse(
            compressed_text=result.get("compressed_prompt", text),
            original_tokens=original,
            compressed_tokens=compressed,
            compression_ratio=round(ratio, 2),
            latency_ms=latency_ms,
            status="compressed",
            warmup=warmup,
        )
    except Exception as e:
        latency_ms = int((time.time() - start) * 1000)
        return CompressResponse(
            compressed_text=text,
            original_tokens=0,
            compressed_tokens=0,
            compression_ratio=1.0,
            latency_ms=latency_ms,
            status="skipped",
        )


def _compress_messages(
    compressor, messages: List[Dict[str, Any]], rates: Dict[str, float],
    force_tokens: List[str], chunk_end_tokens: List[str],
    start: float, warmup: bool
) -> CompressResponse:
    """Compress OpenAI-style messages with per-role rates."""
    default_rate = rates.get("default", 0.3)
    compressed_messages = []
    total_original = 0
    total_compressed = 0

    for msg in messages:
        role = msg.get("role", "user")
        content = msg.get("content", "")
        rate = rates.get(role, default_rate)

        try:
            result = compressor.compress_prompt_llmlingua2(
                str(content),
                rate=rate,
                force_tokens=force_tokens,
                chunk_end_tokens=chunk_end_tokens,
                return_word_label=False,
                drop_consecutive=True,
            )
            compressed_messages.append({
                "role": role,
                "content": result.get("compressed_prompt", content),
            })
            total_original += result.get("origin_tokens", 0)
            total_compressed += result.get("compressed_tokens", 0)
        except Exception:
            compressed_messages.append({"role": role, "content": content})

    latency_ms = int((time.time() - start) * 1000)
    ratio = total_original / total_compressed if total_compressed > 0 else 1.0
    return CompressResponse(
        compressed_messages=compressed_messages,
        original_tokens=total_original,
        compressed_tokens=total_compressed,
        compression_ratio=round(ratio, 2),
        latency_ms=latency_ms,
        status="compressed",
        warmup=warmup,
    )


if __name__ == "__main__":
    import uvicorn
    port = int(os.environ.get("COMPACTION_PORT", "18962"))
    uvicorn.run(app, host="127.0.0.1", port=port)
