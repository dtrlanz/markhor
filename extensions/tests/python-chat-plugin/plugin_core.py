# plugin_core.py (Updated)
import sys
import json
import logging
import abc
from typing import Dict, Any, Callable

# --- Communication Protocol Overview ---
# This script communicates with the host process (Rust) via standard streams:
# - Standard Input (stdin): Receives a single JSON object containing the method to call
#                           and its parameters (e.g., {"method": "chat", "params": {...}}).
# - Standard Output (stdout): Sends a single JSON object representing the result.
#                             On success: {"status": "success", "result": {...}}
#                             On error:   {"status": "error", "message": "..."}
#                             *** stdout is ONLY used for this final JSON result. ***
# - Standard Error (stderr): Used EXCLUSIVELY for logging diagnostic information
#                            (via the Python `logging` module). This helps debug
#                            the plugin without interfering with the JSON on stdout.

# --- Logging Configuration ---
# Configure the root logger.
# IMPORTANT: Log messages are directed to stderr to keep stdout clean for JSON results.
logging.basicConfig(
    level=logging.INFO,           # Minimum level to process (INFO, WARNING, ERROR, CRITICAL)
    stream=sys.stderr,            # *** Direct log output to Standard Error ***
    format='%(levelname)s:%(name)s:%(message)s' # Log message format
)
log = logging.getLogger("plugin_core") # Get logger for this module

# --- Base Class ---
class BasePlugin(abc.ABC):

    @abc.abstractmethod
    def get_method_handlers(self) -> Dict[str, Callable[[Dict[str, Any]], Dict[str, Any]]]:
        """
        Plugin authors must implement this method.
        It should return a dictionary mapping method names (strings matching the
        'method' field in the input JSON) to the actual plugin methods
        that handle them.

        Each handler method must accept a single dictionary argument (the 'params'
        from the input JSON) and return a dictionary representing the successful
        result (which will be placed under the 'result' key in the output JSON).

        Example:
            return {
                "chat": self.handle_chat_request,
                "embed": self.handle_embed_request,
            }
        """
        pass

    def run(self):
        """
        Handles the main execution loop:
        1. Reads and parses the JSON request from stdin.
        2. Identifies the target method from the request.
        3. Calls the appropriate handler method implemented by the subclass.
        4. Catches exceptions during handler execution.
        5. Writes the final JSON response (success or error) to stdout.
        6. Sets the exit code (0 for success, 1 for error).
        """

        input_data = None
        method_name = None
        params = None
        handlers = {}

        try:
            # Pre-fetch handlers to catch implementation errors early
            handlers = self.get_method_handlers()
            if not isinstance(handlers, dict):
                 raise TypeError("get_method_handlers must return a dict")

            input_str = sys.stdin.read()
            if not input_str:
                 log.error("Received empty input from stdin.")
                 self._write_output({"status": "error", "message": "No input received."})
                 sys.exit(1)

            log.info(f"Received input string length: {len(input_str)}")
            input_data = json.loads(input_str)

            if not isinstance(input_data, dict):
                raise ValueError("Input must be a JSON object.")

            method_name = input_data.get("method")
            params = input_data.get("params", {}) # Default to empty dict if params is missing

            if not method_name:
                raise ValueError("Input JSON must include a 'method' field.")
            if not isinstance(params, dict):
                 raise ValueError("'params' field must be an object/dictionary.")

        except json.JSONDecodeError:
            log.error("Failed to decode JSON from stdin.", exc_info=True)
            self._write_output({"status": "error", "message": "Invalid JSON input."})
            sys.exit(1)
        except (ValueError, TypeError) as e:
            log.error(f"Invalid input structure: {e}", exc_info=True)
            self._write_output({"status": "error", "message": f"Invalid input structure: {e}"})
            sys.exit(1)
        except Exception as e:
             # Catch other potential errors during setup/parsing
             log.error(f"Error processing input: {e}", exc_info=True)
             self._write_output({"status": "error", "message": f"Error processing input: {e}"})
             sys.exit(1)

        # --- Dispatching ---
        handler_func = handlers.get(method_name)

        if handler_func is None or not callable(handler_func):
            log.error(f"No valid handler found for method: '{method_name}'")
            self._write_output({
                "status": "error",
                "message": f"Unsupported method '{method_name}' in this plugin."
            })
            sys.exit(1)

        # --- Execute Handler ---
        output_data = None
        try:
            log.info(f"Dispatching to method '{method_name}'")
            # Call the specific handler method with the params dict
            result_data = handler_func(params)

            if not isinstance(result_data, dict):
                 raise TypeError(f"Handler for method '{method_name}' must return a dict, got {type(result_data)}")

            # Wrap successful result for output
            output_data = {"status": "success", "result": result_data}

        except Exception as e:
            # Catch errors specifically from the plugin's handler logic
            log.error(f"Exception in handler for method '{method_name}': {e}", exc_info=True)
            # Create a standard error response structure for stdout
            output_data = {
                "status": "error",
                "message": f"Error executing method '{method_name}': {e}"
                # Or include error type?
                # "message": f"Error executing method '{method_name}': {type(e).__name__} - {e}"
            }

        # --- Output ---
        # Write the final result (success or structured error) to stdout
        self._write_output(output_data)

        # Exit code based on logical success/failure reported in the output_data
        if output_data.get("status") != "success":
             sys.exit(1) # Exit with non-zero status for errors
        else:
             sys.exit(0) # Exit with zero status for success

    def _write_output(self, data_to_write: Dict[str, Any]):
        """
        Safely serializes the result dictionary to JSON and prints it to stdout.
        This is the primary channel for returning structured data to the host process.
        Includes flushing stdout to ensure the host receives the data promptly.
        """
        try:
            output_json = json.dumps(data_to_write)
            # *** Print final JSON result to Standard Output ***
            print(output_json, flush=True)
            # Log the length to stderr for debugging purposes (optional)
            log.info(f"Sent output string length: {len(output_json)}") # Log diagnostic info to stderr
        except Exception as e:
             # If serialization fails, log the error to stderr
             log.error(f"FATAL: Failed to serialize output data: {data_to_write}", exc_info=True)
             # Try to send a minimal error JSON to stdout as a fallback
             minimal_error = json.dumps({
                 "status": "error",
                 "message": f"Internal plugin error: Failed to serialize response - {e}"
             })
             # *** Print fallback error JSON to Standard Output ***
             print(minimal_error, flush=True)
             # Ensure exit code reflects the serialization failure
             if data_to_write.get("status") != "error":
                  sys.exit(1)


# --- Helper function to run a plugin class ---
def run_plugin(PluginClass: type[BasePlugin]):
     # (Same as before, but ensures it's a BasePlugin subclass)
     if not issubclass(PluginClass, BasePlugin):
         raise TypeError("Provided class is not a subclass of BasePlugin")
     # Could add checks here, e.g., ensure get_method_handlers is implemented
     try:
        plugin_instance = PluginClass()
        plugin_instance.run()
     except Exception as e:
         # Catch errors during plugin instantiation
         log.error(f"Failed to instantiate plugin {PluginClass.__name__}: {e}", exc_info=True)
         # Try to output a final error message if possible
         minimal_error = json.dumps({
            "status": "error",
            "message": f"Failed to initialize plugin: {e}"
         })
         print(minimal_error, flush=True)
         sys.exit(1)