"""Pydantic models for AI provider management and chat."""

from pydantic import BaseModel
from typing import Optional, List


class AIProviderBody(BaseModel):
    provider: str   # 'anthropic' | 'openai'
    label: str
    api_key: str
    model: str
    is_active: bool = True


class UserAIKeyBody(BaseModel):
    provider: str   # 'anthropic' | 'openai'
    api_key: str
    model: Optional[str] = None


class ChatMessage(BaseModel):
    role: str       # 'user' | 'assistant'
    content: str


class ChatRequest(BaseModel):
    messages: List[ChatMessage]
    context: Optional[dict] = None   # { dataset_id?, sql?, action? }
