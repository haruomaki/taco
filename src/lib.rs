pub extern crate serde;
pub extern crate serde_json;
pub extern crate webview2_com;
pub extern crate windows;

pub mod window;

use std::{cell::RefCell, collections::HashMap, fmt, ptr, rc::Rc, sync::mpsc};

use serde::Deserialize;
use serde_json::Value;
use windows::{
    core::*,
    Win32::{
        Foundation::{E_POINTER, HINSTANCE, HWND, PWSTR, RECT, SIZE},
        // Graphics::Gdi,
        System::WinRT::EventRegistrationToken,
        UI::WindowsAndMessaging::*,
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

type BindingCallback = Box<dyn FnMut(Vec<Value>) -> std::result::Result<Value, String>>;
type BindingsMap = HashMap<String, BindingCallback>;

pub struct WebViewBuilder<'a> {
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
    pub autosize: bool,
}

impl<'a> Default for WebViewBuilder<'a> {
    fn default() -> Self {
        Self {
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
            autosize: false,
        }
    }
}

#[derive(Clone)]
pub struct WebView {
    pub controller: ICoreWebView2Controller,
    pub core: ICoreWebView2,
    bindings: Rc<RefCell<BindingsMap>>,
    pub hwnd: HWND,
    pub hwnd_widget0: HWND,
    pub hwnd_widget1: HWND,
    pub hwnd_widgethost: HWND,
    pub hwnd_d3d: HWND,
    pub hinstance: HINSTANCE,
}

#[derive(Debug, Deserialize)]
struct InvokeMessage {
    id: u64,
    method: String,
    params: Vec<Value>,
}

impl<'a> WebViewBuilder<'a> {
    pub fn build<T: 'static>(
        mut self,
    ) -> Result<(WebView, window::WindowRunner<T>, window::WindowHandle<T>)> {
        unsafe {
            use windows::Win32::System::Com::*;
            CoInitializeEx(ptr::null_mut(), COINIT_APARTMENTTHREADED)?;
        }

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

        let (mut wrun, whandle) = window::create_window(
            self.style,
            self.exstyle,
            "WebView",
            self.title,
            self.x,
            self.y,
            self.width,
            self.height,
        );

        let hwnd = whandle.hwnd;
        let hinstance = whandle.hinstance;

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

        unsafe {
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

        fn find_child(hwndparent: HWND, lpszclass: &str) -> HWND {
            unsafe { FindWindowExA(hwndparent, None, lpszclass, None) }
        }
        let hwnd_widget0 = find_child(hwnd, "Chrome_WidgetWin_0");
        let hwnd_widget1 = find_child(hwnd_widget0, "Chrome_WidgetWin_1");
        let hwnd_widgethost = find_child(hwnd_widget1, "Chrome_RenderWidgetHostHWND");
        // let hwnd_d3d = find_child(hwnd_widget1, "Intermediate D3D Window");  doesn't work

        let mut webview = WebView {
            controller,
            core,
            bindings: Rc::new(RefCell::new(HashMap::new())),
            hwnd,
            hwnd_widget0,
            hwnd_widget1,
            hwnd_widgethost,
            hwnd_d3d: HWND(0),
            hinstance,
        };

        // Inject the invoke handler.
        webview
            .init(r#"window.external = { invoke: s => window.chrome.webview.postMessage(s) };"#)?;

        unsafe {
            let w = webview.clone();
            let mut _token = EventRegistrationToken::default();
            webview.core.WebMessageReceived(
                WebMessageReceivedEventHandler::create(Box::new(
                    move |_core, args: Option<ICoreWebView2WebMessageReceivedEventArgs>| {
                        if let Some(args) = args {
                            let mut message = PWSTR::default();
                            if args.WebMessageAsJson(&mut message).is_ok() {
                                let message = take_pwstr(message);
                                if let Ok(value) = serde_json::from_str::<InvokeMessage>(&message) {
                                    let mut bindings = w.bindings.borrow_mut();
                                    if let Some(f) = bindings.get_mut(&value.method) {
                                        let webview = &w;
                                        window::dispatch_unsafe(hwnd, move |_: &T| {
                                            match f(value.params) {
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

        if self.autosize {
            let w = webview.clone();
            webview.bind_unsafe("_rpc_adjustWindowToContent", move |request| {
                if let [width, height] = &request[..] {
                    let width = width.as_f64().unwrap();
                    let height = height.as_f64().unwrap();
                    // println!("width = {:?}, height = {:?}", width, height);
                    adjust_to_content(&w, width as _, height as _);
                }
                Ok(Value::Null)
            });

            webview.init(include_str!("autosize.js")).unwrap();
        } else {
            let w = webview.clone();
            wrun.add_event_listener(WM_SIZE, move |_, _| {
                let size = get_window_size(hwnd);
                w.set_webview_size(size.cx, size.cy);
            });
            let size = get_window_size(hwnd);
            webview.set_webview_size(size.cx, size.cy);
        }

        if self.transparent {
            webview.bg();
        }

        if !self.url.is_empty() {
            webview.navigate(self.url)?.set_visible(true)?;
        }

        // Here because it needs a delay of about 150 ms or more.
        webview.hwnd_d3d = find_child(hwnd_widget1, "Intermediate D3D Window");

        Ok((webview, wrun, whandle))
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

    pub fn bind_unsafe<F>(&self, name: impl AsRef<str>, f: F)
    where
        F: FnMut(Vec<Value>) -> std::result::Result<Value, String> + 'static,
    {
        let name = name.as_ref();
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

        self.init(&js).unwrap();
    }

    pub fn bind<F>(&self, name: impl AsRef<str>, f: F)
    where
        F: FnMut(Vec<Value>) -> std::result::Result<Value, String> + Send + 'static,
    {
        self.bind_unsafe(name, f);
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

    // ??????????????????
    // TODO: ?????????????????????????????????????????????????????????
    pub fn bg(&self) {
        let t = one_to_two(&self.controller);

        // ?????????
        let backgroundcolor = COREWEBVIEW2_COLOR {
            R: 20,
            G: 200,
            B: 70,
            A: 0,
        };
        unsafe {
            t.SetDefaultBackgroundColor(backgroundcolor).unwrap();
        }

        // ?????????
        let mut backgroundcolor = Default::default();
        unsafe {
            t.DefaultBackgroundColor(&mut backgroundcolor).unwrap();
        }
        println!("{:?}", backgroundcolor);

        unsafe {
            SetLayeredWindowAttributes(self.hwnd, 0x0000FF00, 50, LWA_COLORKEY);
            // SetLayeredWindowAttributes(self.hwnd, 0x0000FF00, 100, LWA_ALPHA);

            //     let hrgndst = Gdi::CreateEllipticRgn(0, 0, 500, 400);

            //     let hrgn = Gdi::CreateRectRgn(-100, -100, 900, 100);
            //     Gdi::CombineRgn(hrgndst, hrgndst, hrgn, Gdi::RGN_OR);

            //     Gdi::SetWindowRgn(self.hwnd, hrgndst, true);

            // Gdi::InvalidateRect(self.hwnd, null(), true).ok().unwrap();
            // Gdi::UpdateWindow(self.hwnd).ok().unwrap();
        }
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

    pub fn set_webview_size(&self, width: i32, height: i32) {
        unsafe {
            self.controller
                .SetBounds(RECT {
                    left: 0,
                    top: 0,
                    right: width,
                    bottom: height,
                })
                .unwrap();
        }
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

pub fn adjust_to_content(webview: &WebView, offset_width: f64, offset_height: f64) {
    let mut window = RECT::default();
    let mut client = RECT::default();

    unsafe {
        GetWindowRect(webview.hwnd, &mut window);
        GetClientRect(webview.hwnd, &mut client);
    }

    let non_client_width = (window.right - window.left) - (client.right - client.left);
    let non_client_height = (window.bottom - window.top) - (client.bottom - client.top);

    let dpi = unsafe { windows::Win32::UI::HiDpi::GetDpiForWindow(webview.hwnd) };
    let ratio = dpi as f64 / 96.;
    let width = (offset_width * ratio) as i32;
    let height = (offset_height * ratio) as i32;
    unsafe {
        SetWindowPos(
            webview.hwnd,
            None,
            0,
            0,
            width + non_client_width,
            height + non_client_height,
            SWP_NOMOVE | SWP_NOZORDER,
        );
    }
    webview.set_webview_size(width, height);
}

fn one_to_two(one: &ICoreWebView2Controller) -> &ICoreWebView2Controller2 {
    unsafe { std::mem::transmute(one) }
}
