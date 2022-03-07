// #![windows_subsystem = "windows"]

use taco::serde_json::{Number, Value};
use taco::WebView;

use std::sync::{Arc, Mutex};
use std::thread::{sleep, spawn};
use std::time::Duration;

fn main() -> taco::Result<()> {
    let webview = WebView::create(true)?;

    let counter = Arc::new(Mutex::new(0));

    // Bind a quick and dirty calculator callback.
    webview.bind("hostCallback", move |request| {
        if let [Value::String(str), Value::Number(a), Value::Number(b)] = &request[..] {
            if str == "Add" {
                let result = a.as_f64().unwrap_or(0f64) + b.as_f64().unwrap_or(0f64);
                let result = Number::from_f64(result);
                if let Some(result) = result {
                    return Ok(Value::Number(result));
                }
            }
        }

        Err(taco::Error::WebView2Error(
            webview2_com::Error::CallbackError(String::from(
                r#"Usage: window.hostCallback("Add", a, b)"#,
            )),
        ))
    })?;

    let count = counter.clone();
    webview.bind("charge", move |request| {
        if let [Value::Number(x)] = &request[..] {
            let mut lock = count.lock().unwrap();
            (*lock) += x.as_i64().unwrap();
            return Ok(Value::Null);
        }

        Err(taco::Error::WebView2Error(
            webview2_com::Error::CallbackError(String::from(r#"Usage: window.charge(x)"#)),
        ))
    })?;

    // // Configure the target URL and add an init script to trigger the calculator callback.
    // webview
    //     .set_title("webview2-com example (crates/webview2-com/examples)")?
    //     .init(
    //         r#"window.hostCallback("Add", 2, 6).then(result => console.log(`Result: ${result}`));"#,
    //     )?
    //     // .navigate("https://github.com/wravery/webview2-rs")?;
    //     .navigate("C:\\Users\\haruo\\projects\\taco\\web\\main.html")?;

    let count = counter.clone();
    let hwnd = webview.get_window();
    spawn(move || loop {
        sleep(Duration::from_millis(1000));
        let lock = count.lock().unwrap();
        println!("カウントは今 {} だよ", lock);
    });

    taco::navigate(
        hwnd,
        String::from("C:\\Users\\haruo\\projects\\taco\\web\\main.html"),
    );

    // Off we go....
    webview.run()
}
