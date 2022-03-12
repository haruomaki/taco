// #![windows_subsystem = "windows"]

use taco::serde_json::{Number, Value};
use taco::WebView;

use taco::windows::Win32::UI::WindowsAndMessaging::*;

use std::sync::{Arc, Mutex};
use std::thread::{sleep, spawn};
use std::time::Duration;

fn main() -> taco::Result<()> {
    let webview = WebView::create(
        WS_OVERLAPPEDWINDOW,
        Default::default(),
        "たいとるです",
        true,
        false,
    )?;

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

    let hwnd = webview.get_window();
    webview.bind("adjustToContent", move |request| {
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
    })?;

    // // Configure the target URL and add an init script to trigger the calculator callback.
    // webview
    //     .set_title("webview2-com example (crates/webview2-com/examples)")?
    //     .init(
    //         r#"window.hostCallback("Add", 2, 6).then(result => console.log(`Result: ${result}`));"#,
    //     )?
    //     // .navigate("https://github.com/wravery/webview2-rs")?;
    //     .navigate("C:\\Users\\haruo\\projects\\taco\\web\\main.html")?;

    // let count = counter.clone();
    // spawn(move || loop {
    //     sleep(Duration::from_millis(1000));
    //     let lock = count.lock().unwrap();
    //     println!("カウントは今 {} だよ", lock);
    // });

    // spawn(move || {
    //     sleep(Duration::from_millis(1000));
    taco::dispatch(hwnd, |webview| {
        webview
            .bind("み", |_request| {
                if let Some(result) = Number::from_f64(333.) {
                    return Ok(Value::Number(result));
                }

                Err(taco::Error::WebView2Error(
                    webview2_com::Error::CallbackError(String::from(r#"Usage: window.み()"#)),
                ))
            })
            .unwrap();
    });
    // });

    webview.navigate(String::from(
        "C:\\Users\\haruo\\projects\\taco\\web\\main.html",
    ));

    webview
        .eval("adjustToContent(document.body.scrollWidth, document.body.scrollHeight)")
        .unwrap();

    // Off we go....
    webview.run()
}
