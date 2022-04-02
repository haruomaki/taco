use crate::Result;
use crate::{GetWindowLong, SetWindowLong};

use std::ffi::CString;
use std::marker::PhantomData;
use std::ptr::null;

use windows::Win32::{
    Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, PSTR, RECT, WPARAM},
    System::LibraryLoader::GetModuleHandleA,
    UI::HiDpi,
    UI::WindowsAndMessaging::*,
};

type WndProc = Box<dyn FnMut(HWND, u32, WPARAM, LPARAM) -> LRESULT>;

pub struct WindowRunner<T> {
    hwnd: HWND,
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
        let p = GetWindowLong(hwnd, GWLP_USERDATA) as *mut WndProc;
        match p.as_mut() {
            Some(f) => f(hwnd, msg, wparam, lparam),
            None => DefWindowProcA(hwnd, msg, wparam, lparam),
        }
    }
}

#[allow(non_snake_case)]
pub fn DefWindowProc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_DPICHANGED => unsafe {
            let rect = *(lparam.0 as *mut RECT);
            let x = rect.left;
            let y = rect.top;
            let w = rect.right - x;
            let h = rect.bottom - y;
            SetWindowPos(hwnd, None, x, y, w, h, Default::default());
            LRESULT::default()
        },

        WM_CLOSE => unsafe {
            DestroyWindow(hwnd);
            LRESULT::default()
        },

        WM_DESTROY => unsafe {
            PostQuitMessage(0);
            LRESULT::default()
        },

        _ => unsafe { DefWindowProcA(hwnd, msg, wparam, lparam) },
    }
}

impl<T> WindowRunner<T> {
    pub fn run(
        self,
        mut wndproc: impl FnMut(HWND, u32, WPARAM, LPARAM) -> LRESULT,
        luggage: T,
    ) -> Result<()> {
        let f = move |hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM| {
            if msg == WM_APP {
                unsafe {
                    let p = lparam.0 as *mut Box<dyn FnOnce(&T) -> Result<()>>;
                    let f = Box::from_raw(p);
                    f(&luggage).unwrap();
                }
            }
            wndproc(hwnd, msg, wparam, lparam)
        };

        let mut f = Box::new(f) as Box<dyn FnMut(HWND, u32, WPARAM, LPARAM) -> LRESULT>;
        let p = &mut f as *mut Box<dyn FnMut(HWND, u32, WPARAM, LPARAM) -> LRESULT>;
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

pub fn create_window<T>(
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

    let wrun = WindowRunner {
        hwnd,
        luggage_type: PhantomData,
    };

    let whandle = WindowHandle {
        hwnd,
        hinstance,
        luggage_type: PhantomData,
    };

    (wrun, whandle)
}
