use crate::extension::Functionality;



pub trait Tool: Functionality {
    fn name(&self) -> &str {
        <Self as Functionality>::name(self)
    }
    fn description(&self) -> String;
    fn parameters(&self) -> serde_json::Value;
    fn execute(&self, arguments: serde_json::Value) -> Result<serde_json::Value, String>;
}