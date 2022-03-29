pub extern crate serde;
pub extern crate serde_json;
pub extern crate webview2_com;
pub extern crate windows;

use std::{collections::HashMap, ffi::CString, fmt, ptr, sync::mpsc};

use serde::Deserialize;
use serde_json::Value;
use windows::{
    core::*,
    Win32::{
        Foundation::{E_POINTER, HWND, LPARAM, LRESULT, PSTR, PWSTR, RECT, SIZE, WPARAM},
        System::WinRT::EventRegistrationToken,
        UI::{HiDpi, WindowsAndMessaging::*},
    },
};

use webview2_com::{Microsoft::Web::WebView2::Win32::*, *};

#[derive(Debug)]
pub enum Error {
    WebView2Error(webview2_com::Error),
    WindowsError(windows::core::Error),
    JsonError(serde_json::Error),
    LockError,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{:?}", self)
    }
}

impl From<webview2_com::Error> for Error {
    fn from(err: webview2_com::Error) -> Self {
        Self::WebView2Error(err)
    }
}

impl From<windows::core::Error> for Error {
    fn from(err: windows::core::Error) -> Self {
        Self::WindowsError(err)
    }
}

impl From<HRESULT> for Error {
    fn from(err: HRESULT) -> Self {
        Self::WindowsError(windows::core::Error::fast_error(err))
    }
}

impl From<serde_json::Error> for Error {
    fn from(err: serde_json::Error) -> Self {
        Self::JsonError(err)
    }
}

impl<'a, T: 'a> From<std::sync::PoisonError<T>> for Error {
    fn from(_: std::sync::PoisonError<T>) -> Self {
        Self::LockError
    }
}

impl<'a, T: 'a> From<std::sync::TryLockError<T>> for Error {
    fn from(_: std::sync::TryLockError<T>) -> Self {
        Self::LockError
    }
}

pub type Result<T> = std::result::Result<T, Error>;

struct Window(HWND);

impl Drop for Window {
    fn drop(&mut self) {
        unsafe {
            DestroyWindow(self.0);
        }
    }
}

type WndProc = Box<dyn FnMut(HWND, u32, WPARAM, LPARAM, &WebView) -> LRESULT>;
type BindingCallback = Box<dyn FnMut(&WebView, Vec<Value>) -> std::result::Result<Value, String>>;
type BindingsMap = HashMap<String, BindingCallback>;

pub struct WebViewBuilder<'a> {
    pub wndproc: WndProc,
    pub style: WINDOW_STYLE,
    pub exstyle: WINDOW_EX_STYLE,
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    pub title: &'a str,
    pub url: &'a str,
    pub debug: bool,
    pub frameless: bool,
    pub resizable: bool,
    pub transparent: bool,
    pub bindings: BindingsMap,
}

impl<'a> Default for WebViewBuilder<'a> {
    fn default() -> Self {
        Self {
            wndproc: Box::new(WebViewDefWindowProc),
            style: WS_OVERLAPPEDWINDOW,
            exstyle: WINDOW_EX_STYLE::default(),
            x: CW_USEDEFAULT,
            y: CW_USEDEFAULT,
            width: CW_USEDEFAULT,
            height: CW_USEDEFAULT,
            title: "",
            url: "",
            debug: true,
            frameless: false,
            resizable: true,
            transparent: false,
            bindings: HashMap::new(),
        }
    }
}

#[derive(Clone)]
pub struct WebView {
    pub controller: ICoreWebView2Controller,
    pub core: ICoreWebView2,
    pub hwnd: HWND,
}

pub struct WebViewRunner {
    pub user_data: Box<(WndProc, WebView)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WebViewHandle {
    pub hwnd: HWND,
}

#[derive(Debug, Deserialize)]
struct InvokeMessage {
    id: u64,
    method: String,
    params: Vec<Value>,
}

impl<'a> WebViewBuilder<'a> {
    pub fn build(mut self) -> Result<(WebViewRunner, WebViewHandle)> {
        unsafe {
            use windows::Win32::System::Com::*;
            CoInitializeEx(ptr::null_mut(), COINIT_APARTMENTTHREADED)?;
        }

        unsafe {
            HiDpi::SetThreadDpiAwarenessContext(HiDpi::DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        }

        let hwnd = {
            let class_name = "WebView";
            let c_class_name = CString::new(class_name).expect("lpszClassName");
            let window_class = WNDCLASSA {
                lpfnWndProc: Some(window_proc),
                lpszClassName: PSTR(c_class_name.as_ptr() as *mut _),
                ..WNDCLASSA::default()
            };

            if self.frameless {
                self.style &= !WS_OVERLAPPEDWINDOW;
                self.style |= WS_POPUP | WS_THICKFRAME;
            }

            if !self.resizable {
                self.style &= !WS_THICKFRAME;
            }

            if self.transparent {
                self.exstyle |= WS_EX_LAYERED
            }

            unsafe {
                RegisterClassA(&window_class);

                let dpi = HiDpi::GetDpiForSystem();
                let ratio = dpi as f64 / 96.;
                let width = self.width as f64 * ratio;
                let height = self.height as f64 * ratio;

                CreateWindowExA(
                    self.exstyle,
                    class_name,
                    self.title,
                    self.style,
                    self.x,
                    self.y,
                    width as i32,
                    height as i32,
                    None,
                    None,
                    None,
                    ptr::null_mut(),
                )
            }
        };

        let wvh = WebViewHandle { hwnd };

        let environment = {
            let (tx, rx) = mpsc::channel();

            CreateCoreWebView2EnvironmentCompletedHandler::wait_for_async_operation(
                Box::new(|environmentcreatedhandler| unsafe {
                    CreateCoreWebView2Environment(environmentcreatedhandler)
                        .map_err(webview2_com::Error::WindowsError)
                }),
                Box::new(
                    move |error_code, environment: Option<ICoreWebView2Environment>| {
                        error_code?;
                        tx.send(
                            environment.ok_or_else(|| windows::core::Error::fast_error(E_POINTER)),
                        )
                        .expect("send over mpsc channel");
                        Ok(())
                    },
                ),
            )?;

            rx.recv()
                .map_err(|_| Error::WebView2Error(webview2_com::Error::SendError))?
        }?;

        let controller = {
            let (tx, rx) = mpsc::channel();

            CreateCoreWebView2ControllerCompletedHandler::wait_for_async_operation(
                Box::new(move |handler| unsafe {
                    environment
                        .CreateCoreWebView2Controller(hwnd, handler)
                        .map_err(webview2_com::Error::WindowsError)
                }),
                Box::new(
                    move |error_code, controller: Option<ICoreWebView2Controller>| {
                        error_code?;
                        tx.send(
                            controller.ok_or_else(|| windows::core::Error::fast_error(E_POINTER)),
                        )
                        .expect("send over mpsc channel");
                        Ok(())
                    },
                ),
            )?;

            rx.recv()
                .map_err(|_| Error::WebView2Error(webview2_com::Error::SendError))?
        }?;

        let size = get_window_size(hwnd);
        let mut client_rect = RECT::default();
        unsafe {
            GetClientRect(hwnd, std::mem::transmute(&mut client_rect));
            controller.SetBounds(RECT {
                left: 0,
                top: 0,
                right: size.cx,
                bottom: size.cy,
            })?;
            controller.SetIsVisible(true)?;
        }

        let core = unsafe { controller.CoreWebView2()? };

        if !self.debug {
            unsafe {
                let settings = core.Settings()?;
                settings.SetAreDefaultContextMenusEnabled(false)?;
                settings.SetAreDevToolsEnabled(false)?;
            }
        }

        let user_data = Box::new((
            self.wndproc,
            WebView {
                controller,
                core,
                hwnd,
            },
        ));

        let webview = &user_data.1;

        // Inject the invoke handler.
        webview
            .init(r#"window.external = { invoke: s => window.chrome.webview.postMessage(s) };"#)?;

        for name in self.bindings.keys() {
            let js = String::from(
                r#"
            (function() {
                var name = '"#,
            ) + name
                + r#"';
                var RPC = window._rpc = (window._rpc || {nextSeq: 1});
                window[name] = function() {
                    var seq = RPC.nextSeq++;
                    var promise = new Promise(function(resolve, reject) {
                        RPC[seq] = {
                            resolve: resolve,
                            reject: reject,
                        };
                    });
                    window.external.invoke({
                        id: seq,
                        method: name,
                        params: Array.prototype.slice.call(arguments),
                    });
                    return promise;
                }
            })()"#;

            webview.init(&js)?;
        }

        unsafe {
            let mut _token = EventRegistrationToken::default();
            webview.core.WebMessageReceived(
                WebMessageReceivedEventHandler::create(Box::new(
                    move |_core, args: Option<ICoreWebView2WebMessageReceivedEventArgs>| {
                        if let Some(args) = args {
                            let mut message = PWSTR::default();
                            if args.WebMessageAsJson(&mut message).is_ok() {
                                let message = take_pwstr(message);
                                if let Ok(value) = serde_json::from_str::<InvokeMessage>(&message) {
                                    if let Some(f) = self.bindings.get_mut(&value.method) {
                                        dispatch_unsafe(hwnd, move |webview| {
                                            match (*f)(webview, value.params) {
                                                Ok(result) => resolve(webview, value.id, 0, result),
                                                Err(err) => resolve(
                                                    webview,
                                                    value.id,
                                                    1,
                                                    Value::String(err),
                                                ),
                                            }
                                        })
                                    }
                                }
                            }
                        }
                        Ok(())
                    },
                )),
                &mut _token,
            )?;
        }

        if self.transparent {
            webview.bg();
        }

        if !self.url.is_empty() {
            webview.navigate(self.url)?;
            wvh.show();
        }

        Ok((WebViewRunner { user_data }, WebViewHandle { hwnd }))
    }

    pub fn bind<F>(mut self, name: &str, f: F) -> Self
    where
        F: FnMut(&WebView, Vec<Value>) -> std::result::Result<Value, String> + Send + 'static,
    {
        self.bindings.insert(String::from(name), Box::new(f));
        self
    }
}

impl WebView {
    pub fn init(&self, js: &str) -> Result<&Self> {
        let core = self.core.clone();
        let js = String::from(js);
        AddScriptToExecuteOnDocumentCreatedCompletedHandler::wait_for_async_operation(
            Box::new(move |handler| unsafe {
                core.AddScriptToExecuteOnDocumentCreated(js, handler)
                    .map_err(webview2_com::Error::WindowsError)
            }),
            Box::new(|error_code, _id| error_code),
        )?;
        Ok(self)
    }

    pub fn navigate(&self, url: &str) -> Result<&Self> {
        let core = &self.core;
        let (tx, rx) = mpsc::channel();

        let handler = NavigationCompletedEventHandler::create(Box::new(move |_sender, _args| {
            tx.send(()).expect("send over mpsc channel");
            Ok(())
        }));
        let mut token = EventRegistrationToken::default();
        unsafe {
            core.NavigationCompleted(handler, &mut token)?;
            if let Err(err) = core.Navigate(url) {
                core.RemoveNavigationCompleted(token)?;
                return Err(err.into());
            }
            webview2_com::wait_with_pump(rx)?;
            core.RemoveNavigationCompleted(token)?;
        }
        Ok(self)
    }

    pub fn eval(&self, js: &str) -> Result<&Self> {
        let core = self.core.clone();
        let js = String::from(js);
        ExecuteScriptCompletedHandler::wait_for_async_operation(
            Box::new(move |handler| unsafe {
                core.ExecuteScript(js, handler)
                    .map_err(webview2_com::Error::WindowsError)
            }),
            Box::new(|error_code, _result| error_code),
        )?;
        Ok(self)
    }

    // 背景を透明化
    // TODO: タイトルバーが透明化されないようにする
    pub fn bg(&self) {
        let t = one_to_two(&self.controller);

        // セット
        let backgroundcolor = COREWEBVIEW2_COLOR {
            R: 20,
            G: 200,
            B: 70,
            A: 0,
        };
        unsafe {
            t.SetDefaultBackgroundColor(backgroundcolor).unwrap();
        }

        // ゲット
        let mut backgroundcolor = Default::default();
        unsafe {
            t.DefaultBackgroundColor(&mut backgroundcolor).unwrap();
        }
        println!("{:?}", backgroundcolor);
    }

    pub fn set_position(&self, x: i32, y: i32) -> Result<&Self> {
        unsafe {
            SetWindowPos(self.hwnd, None, x, y, 0, 0, SWP_NOSIZE | SWP_NOZORDER);
        }
        Ok(self)
    }

    pub fn set_visible(&self, visible: bool) -> Result<&Self> {
        unsafe {
            ShowWindow(self.hwnd, if visible { SW_SHOW } else { SW_HIDE });
        }
        Ok(self)
    }

    pub fn set_topmost(&self, topmost: bool) -> Result<&Self> {
        unsafe {
            SetWindowPos(
                self.hwnd,
                if topmost {
                    HWND_TOPMOST
                } else {
                    HWND_NOTOPMOST
                },
                0,
                0,
                0,
                0,
                // SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER,
                SWP_NOMOVE | SWP_NOSIZE,
            );
        }
        Ok(self)
    }
}

impl WebViewRunner {
    pub fn run(self) -> Result<()> {
        let hwnd = self.user_data.1.hwnd;

        let p = Box::into_raw(self.user_data);
        unsafe { SetWindowLong(hwnd, GWLP_USERDATA, p as _) };

        let mut msg = MSG::default();

        loop {
            unsafe {
                let result = GetMessageA(&mut msg, None, 0, 0).0;

                match result {
                    -1 => break Err(windows::core::Error::from_win32().into()),
                    0 => break Ok(()),
                    _ => match msg.message {
                        _ => {
                            TranslateMessage(&msg);
                            DispatchMessageA(&msg);
                        }
                    },
                }
            }
        }
    }
}

impl WebViewHandle {
    pub fn dispatch(&self, f: impl FnOnce(&WebView) -> Result<()> + Send) {
        dispatch_unsafe(self.hwnd, f)
    }

    pub fn show(&self) {
        unsafe { ShowWindow(self.hwnd, SW_SHOW) };
    }
}

extern "system" fn window_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let (wndproc, webview) = unsafe {
        let p = GetWindowLong(hwnd, GWLP_USERDATA) as *mut (WndProc, WebView);
        if p.is_null() {
            return DefWindowProcA(hwnd, msg, wparam, lparam);
        }
        (&mut (*p).0, &(*p).1)
    };

    (*wndproc)(hwnd, msg, wparam, lparam, webview)
}

#[allow(non_snake_case)]
pub fn WebViewDefWindowProc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    webview: &WebView,
) -> LRESULT {
    match msg {
        WM_APP => {
            let p = lparam.0 as *mut Box<dyn FnOnce(&WebView) -> Result<()>>;
            unsafe {
                let fbb = Box::from_raw(p);
                (*fbb)(webview).unwrap();
            }
            LRESULT::default()
        }

        WM_SIZE => {
            let size = get_window_size(hwnd);
            unsafe {
                webview
                    .controller
                    .SetBounds(RECT {
                        left: 0,
                        top: 0,
                        right: size.cx,
                        bottom: size.cy,
                    })
                    .unwrap();
            }
            LRESULT::default()
        }

        WM_DPICHANGED => unsafe {
            let rect = *(lparam.0 as *mut RECT);
            let x = rect.left;
            let y = rect.top;
            let w = rect.right - x;
            let h = rect.bottom - y;
            SetWindowPos(hwnd, None, x, y, w, h, Default::default());
            LRESULT::default()
        },

        WM_CLOSE => {
            unsafe {
                DestroyWindow(hwnd);
            }
            LRESULT::default()
        }

        WM_DESTROY => {
            // webview.terminate().expect("window is gone");
            unsafe { PostQuitMessage(0) };
            LRESULT::default()
        }

        WM_NCDESTROY => unsafe {
            let p = GetWindowLong(hwnd, GWLP_USERDATA) as *mut (WndProc, WebView);
            let _user_data = Box::from_raw(p);
            LRESULT::default()
        },

        _ => unsafe { DefWindowProcA(hwnd, msg, wparam, lparam) },
    }
}

fn get_window_size(hwnd: HWND) -> SIZE {
    let mut client_rect = RECT::default();
    unsafe { GetClientRect(hwnd, std::mem::transmute(&mut client_rect)) };
    SIZE {
        cx: client_rect.right - client_rect.left,
        cy: client_rect.bottom - client_rect.top,
    }
}

#[allow(non_snake_case)]
#[cfg(target_pointer_width = "32")]
unsafe fn SetWindowLong(window: HWND, index: WINDOW_LONG_PTR_INDEX, value: isize) -> isize {
    SetWindowLongA(window, index, value as _) as _
}

#[allow(non_snake_case)]
#[cfg(target_pointer_width = "64")]
unsafe fn SetWindowLong(window: HWND, index: WINDOW_LONG_PTR_INDEX, value: isize) -> isize {
    SetWindowLongPtrA(window, index, value)
}

#[allow(non_snake_case)]
#[cfg(target_pointer_width = "32")]
unsafe fn GetWindowLong(window: HWND, index: WINDOW_LONG_PTR_INDEX) -> isize {
    GetWindowLongA(window, index) as _
}

#[allow(non_snake_case)]
#[cfg(target_pointer_width = "64")]
unsafe fn GetWindowLong(window: HWND, index: WINDOW_LONG_PTR_INDEX) -> isize {
    GetWindowLongPtrA(window, index)
}

pub fn dispatch_unsafe<F>(hwnd: HWND, f: F)
where
    F: FnOnce(&WebView) -> Result<()>,
    // pub fn my_dispatch(hwnd: HWND, f: dyn FnOnce(&WebView))
{
    let fb = Box::new(f) as Box<dyn FnOnce(&WebView) -> Result<()>>;
    let fbb = Box::new(fb);
    let p = Box::into_raw(fbb);
    unsafe { PostMessageA(hwnd, WM_APP, WPARAM(0), LPARAM(p as _)) };
}

pub fn resolve(webview: &WebView, id: u64, status: i32, result: Value) -> Result<()> {
    let result = result.to_string();
    let method = match status {
        0 => "resolve",
        _ => "reject",
    };
    let js = format!(
        r#"
            window._rpc[{}].{}({});
            window._rpc[{}] = undefined;"#,
        id, method, result, id
    );

    webview.eval(&js).unwrap();
    Ok(())
}

pub fn dispatch_eval(hwnd: HWND, js: String) {
    dispatch_unsafe(hwnd, move |webview| {
        webview.eval(&js)?;
        Ok(())
    });
}

pub fn adjust_to_content(hwnd: HWND, body_scroll_width: i32, body_scroll_height: i32) {
    // println!(
    //     "adjust_to_contentが呼ばれたよ！ {:?} {:?}",
    //     body_scroll_width, body_scroll_height
    // );

    unsafe {
        SetWindowPos(
            hwnd,
            None,
            0,
            0,
            body_scroll_width * 2 + 26,  // dpiとクライアント領域の補正
            body_scroll_height * 2 + 71, // TODO: 補正を自動化
            SWP_NOMOVE | SWP_NOZORDER,
        );
    }
}

fn one_to_two(one: &ICoreWebView2Controller) -> &ICoreWebView2Controller2 {
    unsafe { std::mem::transmute(one) }
}
