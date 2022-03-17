// #![windows_subsystem = "windows"]

use taco::serde_json::{Number, Value};
use taco::windows::Win32::Foundation::LRESULT;
use taco::windows::Win32::UI::WindowsAndMessaging::*;

use std::sync::{Arc, Mutex};

fn main() -> taco::Result<()> {
    let mut count = 0;
    let counter = Arc::new(Mutex::new(0));
    let c = counter.clone();
    let (h, _wvh) = taco::WebViewBuilder {
        wndproc: Box::new(move |hwnd, msg, wparam, lparam, webview| match msg {
            WM_KEYDOWN => {
                webview.eval("console.log('ぴゃあ')").unwrap();
                count += 1;
                println!("かー {}", count);
                LRESULT::default()
            }
            _ => taco::WebViewDefWindowProc(hwnd, msg, wparam, lparam, webview),
        }),
        title: "たいとるです",
        url: "C:\\Users\\haruo\\projects\\taco\\web\\main.html",
        ..Default::default()
    }
    .bind("hostCallback", move |_, request| {
        if let [Value::String(str), Value::Number(a), Value::Number(b)] = &request[..] {
            if str == "Add" {
                let result = a.as_f64().unwrap_or(0f64) + b.as_f64().unwrap_or(0f64);
                let result = Number::from_f64(result);
                if let Some(result) = result {
                    return Ok(Value::Number(result));
                }
            }
        }
        Err(r#"Usage: window.hostCallback("Add", a, b)"#.into())
    })
    .bind("charge", move |_, request| {
        if let [Value::Number(x)] = &request[..] {
            let mut lock = c.lock().unwrap();
            (*lock) += x.as_i64().unwrap();
            println!("残高は {} になったよ", lock);
            return Ok(Value::Null);
        }

        Err(r#"Usage: window.charge(x)"#.into())
    })
    .start()?;

    let (h2, _wvh2) = taco::WebViewBuilder {
        x: 1,
        y: 1,
        width: 300,
        height: 300,
        url: "https://qiita.com/takao_mofumofu/items/24c060a1d4f6b3df5c73",
        ..Default::default()
    }
    .start()?;
    h2.join().unwrap()?;

    // Off we go....
    h.join().unwrap()
}
