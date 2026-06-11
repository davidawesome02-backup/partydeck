use zbus::{Connection, interface};
use std::sync::Arc;

struct MathService;

#[interface(name = "com.example.MathService")]
impl MathService {
    /// Multiply two integers
    async fn multiply(&self, a: i32, b: i32) -> i32 {
        let result = a * b;
        println!("Multiply({}, {}) = {}", a, b, result);
        result
    }
}

#[tokio::main]
async fn main() -> zbus::Result<()> {
    let _conn = Connection::session().await?;

    // Register the service
    _conn
        .object_server()
        .at("/com/example/MathService", MathService)?;

    // Request a name on the bus
    _conn.request_name("com.example.MathService").await?;

    println!("DBus service running at: com.example.MathService");
    println!("Available method: Multiply(int32 a, int32 b) -> int32");
    println!("Call with: dbus-send --session --print-reply --dest=com.example.MathService /com/example/MathService com.example.MathService.Multiply int32:5 int32:25");

    // Keep the service running
    loop {
        tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
    }
    tokio::
}
