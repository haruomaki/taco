use crate::Result;
use crate::{GetWindowLong, SetWindowLong};

use std::collections::HashMap;
use std::ffi::CString;
use std::marker::PhantomData;
use std::ptr::null;

use windows::Win32::{
    Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, PSTR, RECT, WPARAM},
    System::LibraryLoader::GetModuleHandleA,
    UI::HiDpi,
    UI::WindowsAndMessaging::*,
};

type WndProcs = HashMap<u32, Vec<Box<dyn FnMut(WPARAM, LPARAM)>>>;

pub struct WindowRunner<T> {
    hwnd: HWND,
    wndprocs: WndProcs,
    luggage_type: PhantomData<fn() -> T>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WindowHandle<T> {
    pub hwnd: HWND,
    pub hinstance: HINSTANCE,
    luggage_type: PhantomData<fn() -> T>,
}

extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        let p = GetWindowLong(hwnd, GWLP_USERDATA) as *mut WndProcs;
        if let Some(wndprocs) = p.as_mut() {
            if let Some(fs) = wndprocs.get_mut(&msg) {
                for f in fs.iter_mut() {
                    f(wparam, lparam);
                }
                return LRESULT::default();
            }
        }
        DefWindowProcA(hwnd, msg, wparam, lparam)
    }
}

impl<T: 'static> WindowRunner<T> {
    pub fn run(mut self, luggage: T) -> Result<()> {
        self.add_event_listener(WM_APP, move |_, lparam| unsafe {
            let p = lparam.0 as *mut Box<dyn FnOnce(&T) -> Result<()>>;
            let f = Box::from_raw(p);
            f(&luggage).unwrap();
        });
        let p = &mut self.wndprocs as *mut WndProcs;
        unsafe { SetWindowLong(self.hwnd, GWLP_USERDATA, p as _) };

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

    pub fn add_event_listener(&mut self, msg: u32, f: impl FnMut(WPARAM, LPARAM) + 'static) {
        if !self.wndprocs.contains_key(&msg) {
            self.wndprocs.insert(msg, Vec::new());
        }
        let fs = self.wndprocs.get_mut(&msg).unwrap();
        let f = Box::new(f) as _;
        fs.push(f);
    }

    pub fn reset_event_listeners(&mut self, msg: u32) {
        self.wndprocs.remove(&msg);
    }
}

pub fn dispatch_unsafe<T>(hwnd: HWND, f: impl FnOnce(&T) -> Result<()>) {
    let f = Box::new(f) as Box<dyn FnOnce(&T) -> Result<()>>;
    let f = Box::new(f);
    let p = Box::into_raw(f);
    unsafe { PostMessageA(hwnd, WM_APP, WPARAM(0), LPARAM(p as _)) };
}

impl<T> WindowHandle<T> {
    pub fn dispatch(&self, f: impl FnOnce(&T) -> Result<()>) {
        dispatch_unsafe(self.hwnd, f)
    }
}

pub fn create_window<T: 'static>(
    style: WINDOW_STYLE,
    exstyle: WINDOW_EX_STYLE,
    class_name: &str,
    title: &str,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
) -> (WindowRunner<T>, WindowHandle<T>) {
    unsafe {
        HiDpi::SetThreadDpiAwarenessContext(HiDpi::DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }

    let c_class_name = CString::new(class_name).expect("lpszClassName");
    let window_class = WNDCLASSA {
        lpfnWndProc: Some(wndproc),
        lpszClassName: PSTR(c_class_name.as_ptr() as *mut _),
        ..WNDCLASSA::default()
    };

    let hinstance = unsafe { GetModuleHandleA(None) };

    let hwnd = unsafe {
        RegisterClassA(&window_class);

        let dpi = HiDpi::GetDpiForSystem();
        let ratio = dpi as f64 / 96.;
        let width = width as f64 * ratio;
        let height = height as f64 * ratio;

        CreateWindowExA(
            exstyle,
            class_name,
            title,
            style,
            x,
            y,
            width as i32,
            height as i32,
            None,
            None,
            hinstance,
            null(),
        )
    };

    let mut wrun = WindowRunner {
        hwnd,
        wndprocs: HashMap::new(),
        luggage_type: PhantomData,
    };

    // wrun.add_event_listener(msg, f)
    wrun.add_event_listener(WM_DPICHANGED, move |_, lparam| unsafe {
        let rect = *(lparam.0 as *mut RECT);
        let x = rect.left;
        let y = rect.top;
        let w = rect.right - x;
        let h = rect.bottom - y;
        SetWindowPos(hwnd, None, x, y, w, h, Default::default());
    });

    wrun.add_event_listener(WM_CLOSE, move |_, _| unsafe {
        DestroyWindow(hwnd);
    });

    wrun.add_event_listener(WM_DESTROY, |_, _| unsafe {
        PostQuitMessage(0);
    });

    let whandle = WindowHandle {
        hwnd,
        hinstance,
        luggage_type: PhantomData,
    };

    (wrun, whandle)
}
