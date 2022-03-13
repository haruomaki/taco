// #![windows_subsystem = "windows"]

use taco::serde_json::{Number, Value};
use taco::WebView;

use taco::windows::Win32::UI::WindowsAndMessaging::*;

use std::sync::{Arc, Mutex};
use std::thread::{sleep, spawn};
use std::time::Duration;

fn main() -> taco::Result<()> {
    let (h, wvh) = taco::WebViewBuilder {
        style: WS_OVERLAPPEDWINDOW,
        exstyle: Default::default(),
        title: "たいとるです",
        debug: true,
        transparent: false,
    }
    .start()?;

    let hwnd = wvh.hwnd;
    let counter = Arc::new(Mutex::new(0));

    wvh.dispatch(move |webview| {
        // Bind a quick and dirty calculator callback.
        webview
            .bind("hostCallback", move |request| {
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
            })
            .unwrap();

        let count = counter.clone();
        webview
            .bind("charge", move |request| {
                if let [Value::Number(x)] = &request[..] {
                    let mut lock = count.lock().unwrap();
                    (*lock) += x.as_i64().unwrap();
                    println!("残高は {} になったよ", lock);
                    return Ok(Value::Null);
                }

                Err(taco::Error::WebView2Error(
                    webview2_com::Error::CallbackError(String::from(r#"Usage: window.charge(x)"#)),
                ))
            })
            .unwrap();

        webview
            .bind("adjustToContent", move |request| {
                // println!("adjustToContentはじめ");
                if let [Value::Number(width), Value::Number(height)] = &request[..] {
                    let width = width.as_i64().unwrap() as i32;
                    let height = height.as_i64().unwrap() as i32;
                    taco::adjust_to_content(hwnd, width, height);
                    return Ok(Value::Null);
                }

                Err(taco::Error::WebView2Error(
                    webview2_com::Error::CallbackError(String::from(r#"Usage: window.charge(x)"#)),
                ))
            })
            .unwrap();

        webview.navigate("C:\\Users\\haruo\\projects\\taco\\web\\main.html");

        webview
            .eval("adjustToContent(document.body.scrollWidth, document.body.scrollHeight)")
            .unwrap();

        wvh.show();
    });

    // Off we go....
    h.join().unwrap()
}
