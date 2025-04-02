# Gemini Chat API plugin example
#
# This is not the best (or easist) way of interacting with chat completion APIs. It's just a
# convenient way to test the plugin system. And something like this could be a viable solution
# for situations where you need specific functionality provided by a Python SDK.

import os
import logging
from typing import Dict, Any, Callable
from plugin_core import BasePlugin, run_plugin # Import the updated base class

try:
    import google.generativeai as genai
    from google.api_core import exceptions as google_exceptions
except ImportError:
     # Let the plugin fail gracefully later if SDK is needed but not found
     genai = None
     google_exceptions = None

log = logging.getLogger("gemini_plugin")

class GeminiPlugin(BasePlugin):

    def __init__(self):
        super().__init__() # Important if base __init__ does anything
        self.api_key = os.environ.get("GOOGLE_API_KEY")
        if not self.api_key:
            # Can log here, but errors related to usage should be in handlers
            log.warning("GOOGLE_API_KEY environment variable not set.")
        if genai:
            # For simplicity, StdioPythonPlugin currently spawns a new process for every call but 
            # if/when that changes, address question of configuration:
            try:
                # Configure once during initialization? Or per call?
                # Per-call might be safer if key can change, but less efficient.
                # Let's configure per-call for now as it's simpler state-wise.
                # genai.configure(api_key=self.api_key) # Option: configure once
                pass
            except Exception as e:
                log.error(f"Initial Gemini configuration failed: {e}")
                # Don't crash init, let handlers fail if SDK unusable

    def get_method_handlers(self) -> Dict[str, Callable[[Dict[str, Any]], Dict[str, Any]]]:
        return {
            "chat": self.handle_chat_request,
            "count_tokens": self.handle_count_tokens_request,
            # Add more methods here: "embed", "generate_text", etc.
        }

    # --- Handler for 'chat' method ---
    def handle_chat_request(self, params: Dict[str, Any]) -> Dict[str, Any]:
        if not self.api_key:
            raise ValueError("API key not configured.") # Raise exceptions for setup/config errors
        if not genai:
            raise ImportError("google-generativeai SDK not available.")

        messages = params.get("messages", [])
        model_name = params.get("model")
        config = params.get("config", {})

        # Todo: Reuse validation/conversion logic (could be private methods)
        if not messages or messages[-1].get("role", "").lower() != "user":
            log.error(f"Invalid 'messages' input: {messages}")
            raise ValueError("Invalid 'messages' structure or last message not from user.")
        last_message_content = messages[-1].get("content")
        if not last_message_content:
            raise ValueError("Last message content is empty.")
        
        if not model_name:
            raise ValueError("Missing 'model' parameter for chat.")

        generation_config = self._parse_generation_config(config)

        try:
            genai.configure(api_key=self.api_key) # Configure per call
            model = genai.GenerativeModel(model_name)
            history = self._convert_messages_to_gemini_history(messages[:-1])
            log.info(f"Starting chat with history length: {len(history)}")

            chat = model.start_chat(history=history)
            response = chat.send_message(
                last_message_content,
                generation_config=generation_config
            )

            usage = self._extract_usage(response)

            log.info("Successfully received chat response from Gemini.")
            # Return *only* the result part
            return {
                "response": {"role": "model", "content": response.text},
                "usage": usage
            }
        except (google_exceptions.PermissionDenied, google_exceptions.Unauthenticated) as e:
            log.warning(f"Gemini API auth error: {e}")
            raise ValueError(f"Gemini API Key is invalid or lacks permissions: {e}") from e
        except google_exceptions.InvalidArgument as e:
            log.warning(f"Invalid argument to Gemini API: {e}")
            raise ValueError(f"Invalid request to Gemini API: {e}") from e
        except Exception as e:
            # Let the base class catch and format other unexpected errors
            log.error(f"Gemini API chat call failed unexpectedly: {e}", exc_info=True)
            raise  # Re-raise for the base class handler

    # --- Handler for 'count_tokens' method ---
    def handle_count_tokens_request(self, params: Dict[str, Any]) -> Dict[str, Any]:
        if not self.api_key:
             raise ValueError("API key not configured.")
        if not genai:
            raise ImportError("google-generativeai SDK not available.")

        text_content = params.get("text")
        model_name = params.get("model", "gemini-pro") # Use appropriate model for tokenization

        if not text_content:
            raise ValueError("Missing 'text' parameter for count_tokens.")

        try:
            genai.configure(api_key=self.api_key) # Configure per call
            model = genai.GenerativeModel(model_name)
            log.info(f"Counting tokens for text length: {len(text_content)}")
            # Note: Gemini's count_tokens often takes structured content ('parts')
            # This simple example assumes text; adjust as needed for real use.
            count_response = model.count_tokens(text_content)

            log.info("Successfully counted tokens.")
            return {
                "token_count": count_response.total_tokens
            }
        except Exception as e:
            log.error(f"Gemini API count_tokens call failed unexpectedly: {e}", exc_info=True)
            raise # Re-raise for the base class handler


    # --- Helper methods (internal) ---
    def _parse_generation_config(self, config: Dict[str, Any]) -> genai.types.GenerationConfig | None:
         # (Similar logic as before to create GenerationConfig)
         generation_config = None
         if config and genai:
             try:
                 allowed_keys = {'temperature', 'top_p', 'top_k', 'max_output_tokens'}
                 filtered_config = {k: v for k, v in config.items() if k in allowed_keys}
                 if filtered_config:
                     generation_config = genai.types.GenerationConfig(**filtered_config)
             except TypeError as e:
                  raise ValueError(f"Invalid generation config: {e}") from e
         return generation_config

    def _convert_messages_to_gemini_history(self, messages):
        # (Same conversion logic as before)
        history = []
        if genai:
            for message in messages:
                role = message.get("role")
                content = message.get("content")
                if role and content:
                    gemini_role = "user" if role.lower() == "user" else "model"
                    history.append({"role": gemini_role, "parts": [{"text": content}]})
                else:
                    log.warning(f"Skipping invalid message format: {message}")
        return history

    def _extract_usage(self, response) -> Dict[str, Any]:
        # (Same usage extraction logic as before)
         usage = {}
         if hasattr(response, 'usage_metadata'):
             usage["prompt_token_count"] = response.usage_metadata.prompt_token_count
             usage["candidates_token_count"] = response.usage_metadata.candidates_token_count
             usage["total_token_count"] = response.usage_metadata.total_token_count
         return usage


# --- Script Entry Point ---
if __name__ == "__main__":
    run_plugin(GeminiPlugin)