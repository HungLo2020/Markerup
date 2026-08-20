#![cfg(target_os = "ios")]

use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_void};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

pub struct WorkspaceSelection {
    pub path: PathBuf,
    pub bookmark: Vec<u8>,
}

type PickerCallback = Box<dyn FnOnce(Result<Option<WorkspaceSelection>, String>)>;

static RESUME_REQUESTED: AtomicBool = AtomicBool::new(false);

unsafe extern "C" {
    fn markerup_ios_present_directory_picker(callback: extern "C" fn(*const c_char, *const u8, usize, *mut c_void), context: *mut c_void);
    fn markerup_ios_resolve_bookmark(data: *const u8, len: usize, path: *mut *mut c_char) -> bool;
    fn markerup_ios_free_string(path: *mut c_char);
    fn markerup_ios_stop_access(path: *const c_char);
    fn markerup_ios_read_file(path: *const c_char, data: *mut *mut u8, len: *mut usize) -> bool;
    fn markerup_ios_write_file(path: *const c_char, data: *const u8, len: usize) -> bool;
    fn markerup_ios_free_data(data: *mut u8, len: usize);
    fn markerup_ios_mutate(path: *const c_char, destination: *const c_char, operation: u8, data: *const u8, len: usize) -> bool;
    fn markerup_ios_install_lifecycle_observers();
}

pub fn install_lifecycle_observers() {
    unsafe { markerup_ios_install_lifecycle_observers(); }
}

pub fn take_resume_request() -> bool { RESUME_REQUESTED.swap(false, Ordering::AcqRel) }

#[unsafe(no_mangle)]
pub extern "C" fn markerup_ios_resume_request() {
    RESUME_REQUESTED.store(true, Ordering::Release);
}

extern "C" fn picker_callback(path: *const c_char, bookmark: *const u8, bookmark_len: usize, context: *mut c_void) {
    let callback: Box<PickerCallback> = unsafe { Box::from_raw(context.cast()) };
    let result = if path.is_null() {
        Err("Could not access the selected workspace".to_string())
    } else {
        let path = unsafe { CStr::from_ptr(path) }.to_string_lossy().into_owned();
        let bookmark = if bookmark.is_null() { Vec::new() } else { unsafe { std::slice::from_raw_parts(bookmark, bookmark_len) }.to_vec() };
        Ok(Some(WorkspaceSelection { path: PathBuf::from(path), bookmark }))
    };
    callback(result);
}

pub fn choose_workspace(callback: impl FnOnce(Result<Option<WorkspaceSelection>, String>) + 'static) {
    let callback: PickerCallback = Box::new(callback);
    let context = Box::into_raw(Box::new(callback));
    unsafe { markerup_ios_present_directory_picker(picker_callback, context.cast()) };
}

pub fn resolve_bookmark(bookmark: &[u8]) -> Result<WorkspaceSelection, String> {
    let mut path = std::ptr::null_mut();
    let resolved = unsafe { markerup_ios_resolve_bookmark(bookmark.as_ptr(), bookmark.len(), &mut path) };
    if !resolved || path.is_null() {
        return Err("The saved workspace permission is no longer valid".to_string());
    }
    let path_text = unsafe { CStr::from_ptr(path) }.to_string_lossy().into_owned();
    unsafe { markerup_ios_free_string(path) };
    Ok(WorkspaceSelection { path: PathBuf::from(path_text), bookmark: bookmark.to_vec() })
}

pub fn stop_access(path: &std::path::Path) {
    let Ok(path) = CString::new(path.to_string_lossy().as_bytes()) else { return };
    unsafe { markerup_ios_stop_access(path.as_ptr()) };
}

pub fn read_file(path: &std::path::Path) -> Result<Vec<u8>, String> {
    let path = CString::new(path.to_string_lossy().as_bytes()).map_err(|_| "invalid workspace path".to_string())?;
    let mut data = std::ptr::null_mut();
    let mut len = 0;
    let ok = unsafe { markerup_ios_read_file(path.as_ptr(), &mut data, &mut len) };
    if !ok || data.is_null() { return Err("coordinated file read failed".to_string()); }
    let bytes = unsafe { std::slice::from_raw_parts(data, len) }.to_vec();
    unsafe { markerup_ios_free_data(data, len) };
    Ok(bytes)
}

pub fn write_file(path: &std::path::Path, contents: &[u8]) -> Result<(), String> {
    let path = CString::new(path.to_string_lossy().as_bytes()).map_err(|_| "invalid workspace path".to_string())?;
    let ok = unsafe { markerup_ios_write_file(path.as_ptr(), contents.as_ptr(), contents.len()) };
    ok.then_some(()).ok_or_else(|| "coordinated file write failed".to_string())
}

pub fn mutate(path: &std::path::Path, destination: Option<&std::path::Path>, operation: u8, data: &[u8]) -> Result<(), String> {
    let path = CString::new(path.to_string_lossy().as_bytes()).map_err(|_| "invalid workspace path".to_string())?;
    let destination = destination.map(|path| CString::new(path.to_string_lossy().as_bytes())).transpose().map_err(|_| "invalid workspace path".to_string())?;
    let destination_ptr = destination.as_ref().map_or(std::ptr::null(), |value| value.as_ptr());
    let ok = unsafe { markerup_ios_mutate(path.as_ptr(), destination_ptr, operation, data.as_ptr(), data.len()) };
    ok.then_some(()).ok_or_else(|| "coordinated workspace mutation failed".to_string())
}
