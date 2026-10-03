//! Browser-side producer bridge. The JavaScript host prepares the worker before starting WASM.
use super::commands::AudioCommand;
use crate::synthesis::Sample;
use std::{
    cell::RefCell,
    collections::HashMap,
    sync::{Arc, Weak},
};
use wasm_bindgen::prelude::*;
#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_namespace = globalThis, js_name = __tunesWorkerAttach, catch)]
    fn attach_js() -> Result<u32, JsValue>;
    #[wasm_bindgen(js_namespace = globalThis, js_name = __tunesWorkerRate)]
    fn rate_js() -> f32;
    #[wasm_bindgen(js_namespace = globalThis, js_name = __tunesWorkerSend, catch)]
    fn send_js(session: u32, json: &str) -> Result<bool, JsValue>;
    #[wasm_bindgen(js_namespace = globalThis, js_name = __tunesWorkerPcm, catch)]
    fn pcm_js(session: u32, key: &str, pcm: &[f32]) -> Result<bool, JsValue>;
    #[wasm_bindgen(js_namespace = globalThis, js_name = __tunesWorkerRemovePcm)]
    fn remove_pcm_js(session: u32, key: &str) -> bool;
    #[wasm_bindgen(js_namespace = globalThis, js_name = __tunesWorkerPlaying)]
    fn playing_js(session: u32, id: &str) -> bool;
    #[wasm_bindgen(js_namespace = globalThis, js_name = __tunesWorkerMonitor)]
    fn monitor_js(session: u32) -> Vec<f32>;
    #[wasm_bindgen(js_namespace = globalThis, js_name = __tunesWorkerEnableMonitor)]
    fn enable_monitor_js(session: u32, enabled: bool);
    #[wasm_bindgen(js_namespace = globalThis, js_name = __tunesWorkerDetach)]
    fn detach_js(session: u32);
}
struct Uploads {
    next: u64,
    samples: HashMap<usize, (Weak<Vec<f32>>, String)>,
}
thread_local! {static UPLOADS:RefCell<HashMap<u32,Uploads>>=RefCell::new(HashMap::new());}
pub fn attach() -> Result<(u32, f32), String> {
    let session = attach_js()
        .map_err(|e| format!("Audio worker was not initialized by the launcher: {e:?}"))?;
    let rate = rate_js();
    if session == 0 || !rate.is_finite() || !(8000.0..=192000.0).contains(&rate) {
        return Err("Invalid worker configuration".into());
    }
    UPLOADS.with(|u| {
        u.borrow_mut().insert(
            session,
            Uploads {
                next: 1,
                samples: HashMap::new(),
            },
        )
    });
    Ok((session, rate))
}
pub fn upload(session: u32, sample: &Sample) -> Result<String, String> {
    UPLOADS.with(|all| {
        let mut all = all.borrow_mut();
        let uploads = all.get_mut(&session).ok_or("worker session closed")?;
        let pointer = Arc::as_ptr(&sample.data) as usize;
        if let Some((weak, key)) = uploads.samples.get(&pointer) {
            if weak.upgrade().is_some() {
                return Ok(key.clone());
            }
        }
        uploads.samples.retain(|_, (weak, key)| {
            if weak.strong_count() == 0 {
                !remove_pcm_js(session, key)
            } else {
                true
            }
        });
        let key = uploads.next.to_string();
        uploads.next += 1;
        if !pcm_js(session, &key, &sample.data).map_err(|e| format!("PCM upload failed: {e:?}"))? {
            return Err("worker upload queue full".into());
        }
        uploads
            .samples
            .insert(pointer, (Arc::downgrade(&sample.data), key.clone()));
        Ok(key)
    })
}
pub fn send(session: u32, command: &AudioCommand) -> Result<(), ()> {
    let result = (|| {
        let wire = super::worker_protocol::encode(command, &mut |sample| upload(session, sample))?;
        let json = serde_json::to_string(&wire).map_err(|e| e.to_string())?;
        if send_js(session, &json).map_err(|e| format!("worker send failed: {e:?}"))? {
            Ok(())
        } else {
            Err("worker command queue full or unavailable".into())
        }
    })();
    result.map_err(|error: String| web_sys::console::error_1(&format!("[tunes] {error}").into()))
}
pub fn is_playing(session: u32, id: u64) -> bool {
    playing_js(session, &id.to_string())
}
pub fn close(session: u32) {
    UPLOADS.with(|all| all.borrow_mut().remove(&session));
    detach_js(session);
}

pub fn monitor(session: u32) -> Vec<f32> {
    monitor_js(session)
}
pub fn enable_monitor(session: u32, enabled: bool) {
    enable_monitor_js(session, enabled)
}
