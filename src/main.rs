// #![windows_subsystem = "windows"]

use taco::serde_json::{Number, Value};
use taco::windows::Win32::UI::WindowsAndMessaging::*;

use std::sync::{Arc, Mutex};
use std::{
    thread::{sleep, spawn},
    time::Duration,
};

fn main() -> taco::Result<()> {
    std::thread::spawn(|| {
        let (_webview2, wrun2, _whandle2) = taco::WebViewBuilder {
            x: 1,
            y: 1,
            width: 300,
            height: 300,
            url: "https://qiita.com/takao_mofumofu/items/24c060a1d4f6b3df5c73",
            ..Default::default()
        }
        .build()?;
        wrun2.run(())
    });

    let mut count = 0;
    let counter = Arc::new(Mutex::new(0));
    let c = counter.clone();
    let (webview, mut wrun, whandle) = taco::WebViewBuilder {
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
    .build()?;

    spawn(move || {
        // // スレッドアンセーフな共有
        // let count = std::rc::Rc::new(std::cell::RefCell::new(0));

        // for _ in 0..1_000 {
        //     *count.borrow_mut() += 1;
        //     let c = count.clone();
        //     whandle.dispatch(move |_| {
        //         *c.borrow_mut() += 1;
        //         Ok(())
        //     });
        // }

        // スレッドセーフな共有
        let count = std::sync::Arc::new(std::sync::Mutex::new(0));

        for _ in 0..1_000 {
            *count.lock().unwrap() += 1;
            let c = count.clone();
            whandle.dispatch(move |x| {
                *c.lock().unwrap() += 1 + *x;
                Ok(())
            });
        }

        sleep(Duration::from_millis(1));

        // wvh.dispatch(move |_| {
        println!("count = {:?}", count);
        //     Ok(())
        // });

        // moveを強制された安全な参照（ムーブ）
        let y = 8181;
        whandle.dispatch(move |_| {
            println!("y = {}", y);
            Ok(())
        });

        // // 'staticが無いと防げない危険な参照
        // let y = 8181;
        // whandle.dispatch(|_| {
        //     println!("y = {}", y);
        //     Ok(())
        // });
    });

    wrun.add_event_listener(WM_KEYDOWN, move |_, _| {
        webview.eval("console.log('ぴゃあ')").unwrap();
        count += 1;
        println!("かー {}", count);
    });

    // Off we go....
    wrun.run(88888)
}
