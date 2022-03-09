pub extern crate serde;
pub extern crate serde_json;
pub extern crate webview2_com;
pub extern crate windows;

use std::{
    collections::HashMap,
    ffi::CString,
    fmt, mem, ptr,
    sync::{mpsc, Arc, Mutex},
};

use serde::Deserialize;
use serde_json::Value;
use windows::{
    core::*,
    Win32::{
        Foundation::{E_POINTER, HWND, LPARAM, LRESULT, PSTR, PWSTR, RECT, SIZE, WPARAM},
        Graphics::Gdi,
        System::{Com::*, LibraryLoader, Threading, WinRT::EventRegistrationToken},
        UI::{
            HiDpi,
            Input::KeyboardAndMouse,
            WindowsAndMessaging::{self, PostQuitMessage, MSG, WINDOW_LONG_PTR_INDEX, WNDCLASSA},
        },
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
            WindowsAndMessaging::DestroyWindow(self.0);
        }
    }
}

struct WebViewController(ICoreWebView2Controller);

type BindingCallback = Box<dyn FnMut(Vec<Value>) -> Result<Value>>;
type BindingsMap = HashMap<String, BindingCallback>;

#[derive(Clone)]
pub struct WebView {
    controller: Arc<WebViewController>,
    pub webview: Arc<ICoreWebView2>,
    thread_id: u32,
    bindings: Arc<Mutex<BindingsMap>>,
    hwnd: HWND,
    parent: Arc<HWND>,
}

impl Drop for WebViewController {
    fn drop(&mut self) {
        unsafe { self.0.Close() }.unwrap();
    }
}

#[derive(Debug, Deserialize)]
struct InvokeMessage {
    id: u64,
    method: String,
    params: Vec<Value>,
}

impl WebView {
    pub fn create(debug: bool) -> Result<WebView> {
        unsafe {
            CoInitializeEx(ptr::null_mut(), COINIT_APARTMENTTHREADED)?;
        }
        set_process_dpi_awareness()?;

        let hwnd = {
            let class_name = "WebView";
            let c_class_name = CString::new(class_name).expect("lpszClassName");
            let window_class = WNDCLASSA {
                lpfnWndProc: Some(window_proc),
                lpszClassName: PSTR(c_class_name.as_ptr() as *mut _),
                ..WNDCLASSA::default()
            };

            unsafe {
                WindowsAndMessaging::RegisterClassA(&window_class);

                WindowsAndMessaging::CreateWindowExA(
                    WindowsAndMessaging::WS_EX_LAYERED,
                    class_name,
                    class_name,
                    WindowsAndMessaging::WS_OVERLAPPEDWINDOW,
                    WindowsAndMessaging::CW_USEDEFAULT,
                    WindowsAndMessaging::CW_USEDEFAULT,
                    WindowsAndMessaging::CW_USEDEFAULT,
                    WindowsAndMessaging::CW_USEDEFAULT,
                    None,
                    None,
                    LibraryLoader::GetModuleHandleA(None),
                    ptr::null_mut(),
                )
            }
        };

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
            WindowsAndMessaging::GetClientRect(hwnd, std::mem::transmute(&mut client_rect));
            controller.SetBounds(RECT {
                left: 0,
                top: 0,
                right: size.cx,
                bottom: size.cy,
            })?;
            controller.SetIsVisible(true)?;
        }

        let webview = unsafe { controller.CoreWebView2()? };

        if !debug {
            unsafe {
                let settings = webview.Settings()?;
                settings.SetAreDefaultContextMenusEnabled(false)?;
                settings.SetAreDevToolsEnabled(false)?;
            }
        }

        let thread_id = unsafe { Threading::GetCurrentThreadId() };

        let webview = WebView {
            controller: Arc::new(WebViewController(controller)),
            webview: Arc::new(webview),
            thread_id,
            bindings: Arc::new(Mutex::new(HashMap::new())),
            hwnd,
            parent: Arc::new(hwnd),
        };

        // Inject the invoke handler.
        webview
            .init(r#"window.external = { invoke: s => window.chrome.webview.postMessage(s) };"#)?;

        let bindings = webview.bindings.clone();
        unsafe {
            let mut _token = EventRegistrationToken::default();
            webview.webview.WebMessageReceived(
                WebMessageReceivedEventHandler::create(Box::new(move |_webview, args| {
                    if let Some(args) = args {
                        let mut message = PWSTR::default();
                        if args.WebMessageAsJson(&mut message).is_ok() {
                            let message = take_pwstr(message);
                            if let Ok(value) = serde_json::from_str::<InvokeMessage>(&message) {
                                if let Ok(mut bindings) = bindings.try_lock() {
                                    if let Some(f) = bindings.get_mut(&value.method) {
                                        match (*f)(value.params) {
                                            Ok(result) => resolve(hwnd, value.id, 0, result),
                                            Err(err) => resolve(
                                                hwnd,
                                                value.id,
                                                1,
                                                Value::String(format!("{:#?}", err)),
                                            ),
                                        }
                                    }
                                }
                            }
                        }
                    }
                    Ok(())
                })),
                &mut _token,
            )?;
        }

        WebView::set_window_webview(hwnd, Some(Box::new(webview.clone())));

        Ok(webview)
    }

    pub fn run(self) -> Result<()> {
        let hwnd = self.hwnd;
        unsafe {
            WindowsAndMessaging::ShowWindow(hwnd, WindowsAndMessaging::SW_SHOW);
            Gdi::UpdateWindow(hwnd);
            KeyboardAndMouse::SetFocus(hwnd);
        }

        let mut msg = MSG::default();
        let h_wnd = HWND::default();

        loop {
            unsafe {
                let result = WindowsAndMessaging::GetMessageA(&mut msg, h_wnd, 0, 0).0;

                match result {
                    -1 => break Err(windows::core::Error::from_win32().into()),
                    0 => break Ok(()),
                    _ => match msg.message {
                        _ => {
                            WindowsAndMessaging::TranslateMessage(&msg);
                            WindowsAndMessaging::DispatchMessageA(&msg);
                        }
                    },
                }
            }
        }
    }

    pub fn set_title(&self, title: &str) -> Result<&Self> {
        unsafe {
            WindowsAndMessaging::SetWindowTextA(self.hwnd, title);
        }
        Ok(self)
    }

    pub fn set_size(&self, width: i32, height: i32) -> Result<&Self> {
        unsafe {
            self.controller.0.SetBounds(RECT {
                left: 0,
                top: 0,
                right: width,
                bottom: height,
            })?;

            WindowsAndMessaging::SetWindowPos(
                self.hwnd,
                None,
                0,
                0,
                width,
                height,
                WindowsAndMessaging::SWP_NOACTIVATE
                    | WindowsAndMessaging::SWP_NOZORDER
                    | WindowsAndMessaging::SWP_NOMOVE,
            );
        }
        Ok(self)
    }

    pub fn get_window(&self) -> HWND {
        *self.parent
    }

    pub fn init(&self, js: &str) -> Result<&Self> {
        let webview = self.webview.clone();
        let js = String::from(js);
        AddScriptToExecuteOnDocumentCreatedCompletedHandler::wait_for_async_operation(
            Box::new(move |handler| unsafe {
                webview
                    .AddScriptToExecuteOnDocumentCreated(js, handler)
                    .map_err(webview2_com::Error::WindowsError)
            }),
            Box::new(|error_code, _id| error_code),
        )?;
        Ok(self)
    }

    pub fn navigate(&self, url: String) {
        let webview = self.webview.as_ref();
        let (tx, rx) = mpsc::channel();

        let handler = NavigationCompletedEventHandler::create(Box::new(move |_sender, _args| {
            tx.send(()).expect("send over mpsc channel");
            Ok(())
        }));
        let mut token = EventRegistrationToken::default();
        unsafe {
            webview.NavigationCompleted(handler, &mut token).unwrap();
            webview.Navigate(url).unwrap();
            let result = webview2_com::wait_with_pump(rx);
            webview.RemoveNavigationCompleted(token).unwrap();
            result.unwrap();
        }
    }

    pub fn eval(&self, js: &str) -> Result<&Self> {
        let webview = self.webview.clone();
        let js = String::from(js);
        ExecuteScriptCompletedHandler::wait_for_async_operation(
            Box::new(move |handler| unsafe {
                webview
                    .ExecuteScript(js, handler)
                    .map_err(webview2_com::Error::WindowsError)
            }),
            Box::new(|error_code, _result| error_code),
        )?;
        Ok(self)
    }

    pub fn bind<F>(&self, name: &str, f: F) -> Result<&Self>
    where
        F: FnMut(Vec<Value>) -> Result<Value> + 'static,
    {
        self.bindings
            .lock()?
            .insert(String::from(name), Box::new(f));

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

        self.init(&js)
    }

    fn set_window_webview(hwnd: HWND, webview: Option<Box<WebView>>) -> Option<Box<WebView>> {
        unsafe {
            match SetWindowLong(
                hwnd,
                WindowsAndMessaging::GWLP_USERDATA,
                match webview {
                    Some(webview) => Box::into_raw(webview) as _,
                    None => 0_isize,
                },
            ) {
                0 => None,
                ptr => Some(Box::from_raw(ptr as *mut _)),
            }
        }
    }

    fn get_window_webview(hwnd: HWND) -> Option<Box<WebView>> {
        unsafe {
            let data = GetWindowLong(hwnd, WindowsAndMessaging::GWLP_USERDATA);

            match data {
                0 => None,
                _ => {
                    let webview_ptr = data as *mut WebView;
                    let raw = Box::from_raw(webview_ptr);
                    let webview = raw.clone();
                    mem::forget(raw);

                    Some(webview)
                }
            }
        }
    }

    // 背景を透明化
    // TODO: タイトルバーが透明化されないようにする
    pub fn bg(&self) {
        let t = one_to_two(&self.controller.0);

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
}

fn set_process_dpi_awareness() -> Result<()> {
    unsafe { HiDpi::SetProcessDpiAwareness(HiDpi::PROCESS_PER_MONITOR_DPI_AWARE)? };
    Ok(())
}

extern "system" fn window_proc(hwnd: HWND, msg: u32, w_param: WPARAM, l_param: LPARAM) -> LRESULT {
    let webview = match WebView::get_window_webview(hwnd) {
        Some(webview) => webview,
        None => return unsafe { WindowsAndMessaging::DefWindowProcA(hwnd, msg, w_param, l_param) },
    };

    match msg {
        WindowsAndMessaging::WM_APP => {
            let p = l_param.0 as *mut Box<dyn FnOnce(&WebView)>;
            unsafe {
                let fbb = Box::from_raw(p);
                (*fbb)(webview.as_ref());
            }
            LRESULT::default()
        }

        WindowsAndMessaging::WM_SIZE => {
            let size = get_window_size(hwnd);
            unsafe {
                webview
                    .controller
                    .0
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

        WindowsAndMessaging::WM_CLOSE => {
            unsafe {
                WindowsAndMessaging::DestroyWindow(hwnd);
            }
            LRESULT::default()
        }

        WindowsAndMessaging::WM_DESTROY => {
            // webview.terminate().expect("window is gone");
            unsafe { PostQuitMessage(0) };
            LRESULT::default()
        }

        _ => unsafe { WindowsAndMessaging::DefWindowProcA(hwnd, msg, w_param, l_param) },
    }
}

fn get_window_size(hwnd: HWND) -> SIZE {
    let mut client_rect = RECT::default();
    unsafe { WindowsAndMessaging::GetClientRect(hwnd, std::mem::transmute(&mut client_rect)) };
    SIZE {
        cx: client_rect.right - client_rect.left,
        cy: client_rect.bottom - client_rect.top,
    }
}

#[allow(non_snake_case)]
#[cfg(target_pointer_width = "32")]
unsafe fn SetWindowLong(window: HWND, index: WINDOW_LONG_PTR_INDEX, value: isize) -> isize {
    WindowsAndMessaging::SetWindowLongA(window, index, value as _) as _
}

#[allow(non_snake_case)]
#[cfg(target_pointer_width = "64")]
unsafe fn SetWindowLong(window: HWND, index: WINDOW_LONG_PTR_INDEX, value: isize) -> isize {
    WindowsAndMessaging::SetWindowLongPtrA(window, index, value)
}

#[allow(non_snake_case)]
#[cfg(target_pointer_width = "32")]
unsafe fn GetWindowLong(window: HWND, index: WINDOW_LONG_PTR_INDEX) -> isize {
    WindowsAndMessaging::GetWindowLongA(window, index) as _
}

#[allow(non_snake_case)]
#[cfg(target_pointer_width = "64")]
unsafe fn GetWindowLong(window: HWND, index: WINDOW_LONG_PTR_INDEX) -> isize {
    WindowsAndMessaging::GetWindowLongPtrA(window, index)
}

pub fn dispatch<F>(hwnd: HWND, f: F)
where
    F: FnOnce(&WebView),
    // pub fn my_dispatch(hwnd: HWND, f: dyn FnOnce(&WebView))
{
    let fb = Box::new(f) as Box<dyn FnOnce(&WebView)>;
    let fbb = Box::new(fb);
    let p = Box::into_raw(fbb);
    unsafe {
        WindowsAndMessaging::PostMessageA(
            hwnd,
            WindowsAndMessaging::WM_APP,
            WPARAM(0),
            LPARAM(p as _),
        )
    };
}

pub fn resolve(hwnd: HWND, id: u64, status: i32, result: Value) {
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

    dispatch_eval(hwnd, &js);
}

pub fn dispatch_eval(hwnd: HWND, js: &str) {
    dispatch(hwnd, move |webview| {
        webview.eval(js).unwrap();
    });
}

pub fn adjust_to_content(hwnd: HWND, body_scroll_width: i32, body_scroll_height: i32) {
    // println!(
    //     "adjust_to_contentが呼ばれたよ！ {:?} {:?}",
    //     body_scroll_width, body_scroll_height
    // );

    unsafe {
        WindowsAndMessaging::SetWindowPos(
            hwnd,
            None,
            0,
            0,
            body_scroll_width * 2 + 26,  // dpiとクライアント領域の補正
            body_scroll_height * 2 + 71, // TODO: 補正を自動化
            WindowsAndMessaging::SWP_NOMOVE | WindowsAndMessaging::SWP_NOZORDER,
        );
    }
}

fn one_to_two(one: &ICoreWebView2Controller) -> &ICoreWebView2Controller2 {
    unsafe { std::mem::transmute(one) }
}
