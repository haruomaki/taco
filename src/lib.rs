pub extern crate serde;
pub extern crate serde_json;
pub extern crate webview2_com;
pub extern crate windows;

use std::{
    cell::RefCell,
    collections::HashMap,
    ffi::CString,
    fmt, ptr,
    rc::Rc,
    sync::mpsc,
    thread::{spawn, JoinHandle},
};

use serde::Deserialize;
use serde_json::Value;
use windows::{
    core::*,
    Win32::{
        Foundation::{E_POINTER, HWND, LPARAM, LRESULT, PSTR, PWSTR, RECT, SIZE, WPARAM},
        Graphics::Gdi,
        System::{Com::*, LibraryLoader, WinRT::EventRegistrationToken},
        UI::{HiDpi, Input::KeyboardAndMouse, WindowsAndMessaging::*},
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

type BindingCallback = Box<dyn FnMut(Vec<Value>) -> Result<Value>>;
type BindingsMap = HashMap<String, BindingCallback>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WebViewBuilder {
    pub style: WINDOW_STYLE,
    pub exstyle: WINDOW_EX_STYLE,
    pub title: &'static str,
    pub debug: bool,
    pub transparent: bool,
}

#[derive(Clone)]
pub struct WebView {
    pub controller: ICoreWebView2Controller,
    pub core: ICoreWebView2,
    bindings: Rc<RefCell<BindingsMap>>,
    hwnd: HWND,
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

impl WebViewBuilder {
    fn create(mut self) -> Result<HWND> {
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

            if self.transparent {
                self.exstyle |= WS_EX_LAYERED
            }

            unsafe {
                RegisterClassA(&window_class);

                CreateWindowExA(
                    self.exstyle,
                    class_name,
                    self.title,
                    self.style,
                    CW_USEDEFAULT,
                    CW_USEDEFAULT,
                    CW_USEDEFAULT,
                    CW_USEDEFAULT,
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

        let webview = Box::new(WebView {
            controller,
            core,
            bindings: Rc::new(RefCell::new(HashMap::new())),
            hwnd,
        });

        // Inject the invoke handler.
        webview
            .init(r#"window.external = { invoke: s => window.chrome.webview.postMessage(s) };"#)?;

        let bindings = webview.bindings.clone();
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
                                    let mut bindings = bindings.borrow_mut();
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
                        Ok(())
                    },
                )),
                &mut _token,
            )?;
        }

        if self.transparent {
            webview.bg();
        }

        unsafe { SetWindowLong(hwnd, GWLP_USERDATA, Box::into_raw(webview) as _) };

        Ok(hwnd)
    }

    pub fn start(self) -> Result<(JoinHandle<Result<()>>, WebViewHandle)> {
        let (tx, rx) = mpsc::channel();
        let h = spawn(move || {
            let result = self.create();
            match result {
                Ok(hwnd) => tx.send(Ok(hwnd)).unwrap(),
                Err(err) => {
                    tx.send(Err(err)).unwrap();
                    return Ok(());
                }
            }

            let mut msg = MSG::default();
            let h_wnd = HWND::default();

            loop {
                unsafe {
                    let result = GetMessageA(&mut msg, h_wnd, 0, 0).0;

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
        });

        match rx.recv().unwrap() {
            Ok(hwnd) => Ok((h, WebViewHandle { hwnd })),
            Err(err) => Err(err),
        }
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

    pub fn navigate(&self, url: &str) {
        let core = &self.core;
        let (tx, rx) = mpsc::channel();

        let handler = NavigationCompletedEventHandler::create(Box::new(move |_sender, _args| {
            tx.send(()).expect("send over mpsc channel");
            Ok(())
        }));
        let mut token = EventRegistrationToken::default();
        unsafe {
            core.NavigationCompleted(handler, &mut token).unwrap();
            core.Navigate(url).unwrap();
            let result = webview2_com::wait_with_pump(rx);
            core.RemoveNavigationCompleted(token).unwrap();
            result.unwrap();
        }
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

    pub fn bind<F>(&self, name: &str, f: F) -> Result<&Self>
    where
        F: FnMut(Vec<Value>) -> Result<Value> + 'static,
    {
        self.bindings
            .borrow_mut()
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
}

impl WebViewHandle {
    pub fn dispatch(&self, f: impl FnOnce(&WebView)) {
        dispatch(self.hwnd, f)
    }

    pub fn show(&self) {
        unsafe { ShowWindow(self.hwnd, SW_SHOW) };
    }
}

fn set_process_dpi_awareness() -> Result<()> {
    unsafe { HiDpi::SetProcessDpiAwareness(HiDpi::PROCESS_PER_MONITOR_DPI_AWARE)? };
    Ok(())
}

extern "system" fn window_proc(hwnd: HWND, msg: u32, w_param: WPARAM, l_param: LPARAM) -> LRESULT {
    let p = unsafe { GetWindowLong(hwnd, GWLP_USERDATA) } as *mut WebView;
    let webview = unsafe {
        if p.is_null() {
            if msg == WM_APP {
                std::thread::sleep(std::time::Duration::from_millis(1));
                PostMessageA(hwnd, msg, w_param, l_param);
            }
            return DefWindowProcA(hwnd, msg, w_param, l_param);
        }
        &*p
    };

    match msg {
        WM_APP => {
            let p = l_param.0 as *mut Box<dyn FnOnce(&WebView)>;
            unsafe {
                let fbb = Box::from_raw(p);
                (*fbb)(webview);
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
            let _webview = Box::from_raw(p);
            LRESULT::default()
        },

        _ => unsafe { DefWindowProcA(hwnd, msg, w_param, l_param) },
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

pub fn dispatch<F>(hwnd: HWND, f: F)
where
    F: FnOnce(&WebView),
    // pub fn my_dispatch(hwnd: HWND, f: dyn FnOnce(&WebView))
{
    let fb = Box::new(f) as Box<dyn FnOnce(&WebView)>;
    let fbb = Box::new(fb);
    let p = Box::into_raw(fbb);
    unsafe { PostMessageA(hwnd, WM_APP, WPARAM(0), LPARAM(p as _)) };
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

    dispatch_eval(hwnd, js);
}

pub fn dispatch_eval(hwnd: HWND, js: String) {
    dispatch(hwnd, move |webview| {
        webview.eval(&js).unwrap();
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
