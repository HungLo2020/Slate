use slate_core::{App, Command};
use std::{
    ffi::{c_char, c_void, CStr, CString},
    panic::{catch_unwind, AssertUnwindSafe},
};
extern "C" {
    fn slate_qt_run(context: *mut c_void) -> i32;
}
pub fn run(mut app: App) -> i32 {
    // Qt owns the event loop and uses this pointer only on the calling thread.
    // The application outlives that event loop and is dropped afterwards.
    unsafe { slate_qt_run((&mut app as *mut App).cast()) }
}
#[no_mangle]
unsafe extern "C" fn slate_request(context: *mut c_void, request: *const c_char) -> *mut c_char {
    let response=catch_unwind(AssertUnwindSafe(||{
        let app=&mut *context.cast::<App>();
        let request:serde_json::Value=serde_json::from_slice(CStr::from_ptr(request).to_bytes()).unwrap_or_default();
        if request["action"]=="snapshot" {
            let number=|key:&str,default:u16|request[key].as_u64().map(|n|n.min(u16::MAX as u64)as u16).unwrap_or(default);
            serde_json::to_string(&app.snapshot(number("width",1280),number("height",720),6,number("cell_width",9),number("cell_height",18),32)).unwrap()
        }else if request["action"]=="command" {
            app.command_line(request["text"].as_str().unwrap_or(""));
            serde_json::json!({"status":app.status,"quit":app.quit}).to_string()
        }else{
            match serde_json::from_value::<Command>(request){Ok(command)=>app.dispatch(command),Err(e)=>app.status=format!("Invalid command: {e}")};
            serde_json::json!({"status":app.status,"quit":app.quit,"dirty":app.dirty(),"clipboard":app.clipboard}).to_string()
        }
    })).unwrap_or_else(|_|"{\"status\":\"Internal error processing GUI request\"}".into());
    CString::new(response).unwrap().into_raw()
}
#[no_mangle]
unsafe extern "C" fn slate_response_free(response: *mut c_char) {
    if !response.is_null() {
        drop(CString::from_raw(response));
    }
}
