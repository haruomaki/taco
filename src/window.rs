use crate::Result;
use crate::{GetWindowLong, SetWindowLong};

use std::marker::PhantomData;

use windows::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM},
    UI::WindowsAndMessaging::*,
};

struct UserData<T> {
    wndproc: Box<dyn FnMut(HWND, u32, WPARAM, LPARAM, &mut T) -> LRESULT>,
    luggage: T,
}

struct Gift<T> {
    f: Box<dyn FnOnce(&mut T) -> Result<()>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WindowHandle<T> {
    pub hwnd: HWND,
    pub luggage_type: PhantomData<T>,
}

pub extern "system" fn window_proc<T>(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    unsafe {
        let p = GetWindowLong(hwnd, GWLP_USERDATA) as *mut UserData<T>;
        match p.as_mut() {
            Some(UserData { wndproc, luggage }) => wndproc(hwnd, msg, wparam, lparam, luggage),
            None => DefWindowProcA(hwnd, msg, wparam, lparam),
        }
    }
}

#[allow(non_snake_case)]
pub fn DefWindowProc<T>(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    luggage: &mut T,
) -> LRESULT {
    match msg {
        WM_APP => unsafe {
            let p = lparam.0 as *mut Gift<T>;
            let gift = Box::from_raw(p);
            (gift.f)(luggage).unwrap();
            LRESULT::default()
        },

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

        WM_NCDESTROY => unsafe {
            let p = GetWindowLong(hwnd, GWLP_USERDATA) as *mut UserData<T>;
            let _ = Box::from_raw(p);
            LRESULT::default()
        },

        _ => unsafe { DefWindowProcA(hwnd, msg, wparam, lparam) },
    }
}

pub fn run<T>(
    hwnd: HWND,
    wndproc: impl FnMut(HWND, u32, WPARAM, LPARAM, &mut T) -> LRESULT + 'static,
    luggage: T,
) -> Result<()> {
    let wndproc = Box::new(wndproc) as _;
    let user_data = Box::new(UserData { wndproc, luggage });
    let p = Box::into_raw(user_data);
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

pub fn dispatch_unsafe<T>(hwnd: HWND, f: impl FnOnce(&mut T) -> Result<()> + 'static) {
    let f = Box::new(f) as _;
    let gift = Box::new(Gift::<T> { f });
    let p = Box::into_raw(gift);
    unsafe { PostMessageA(hwnd, WM_APP, WPARAM(0), LPARAM(p as _)) };
}

impl<T> WindowHandle<T> {
    pub fn dispatch(&self, f: impl FnOnce(&mut T) -> Result<()> + 'static) {
        dispatch_unsafe(self.hwnd, f)
    }
}
