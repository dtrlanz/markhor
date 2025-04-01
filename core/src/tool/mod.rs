

pub trait Tool {
    fn name(&self) -> String;
    fn description(&self) -> String;
    fn parameters(&self) -> serde_json::Value;
    fn execute(&self, arguments: serde_json::Value) -> Result<serde_json::Value, String>;
}