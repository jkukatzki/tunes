//! Drop retired DSP graphs away from the audio callback.
use super::callback::AudioCallbackState;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

pub(crate) struct Maintenance {
    stopped: Arc<AtomicBool>,
    #[cfg(target_arch = "wasm32")]
    timer: i32,
    #[cfg(target_arch = "wasm32")]
    _callback: wasm_bindgen::closure::Closure<dyn FnMut()>,
}

impl Maintenance {
    pub fn new(state: &AudioCallbackState) -> Self {
        let sounds = state.active_sounds.retired.clone();
        let buses = state.effect_buses.retired.clone();
        #[cfg(not(target_arch = "wasm32"))]
        let streams = state.streaming_sounds.retired.clone();
        let stopped = Arc::new(AtomicBool::new(false));
        #[cfg(not(target_arch = "wasm32"))]
        {
            let stop = stopped.clone();
            std::thread::spawn(move || {
                while !stop.load(Ordering::Acquire) {
                    while sounds.pop().is_some() {}
                    while buses.pop().is_some() {}
                    while streams.pop().is_some() {}
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
                // Remaining queued objects are also reclaimed off the callback.
            });
            Self { stopped }
        }
        #[cfg(target_arch = "wasm32")]
        {
            use wasm_bindgen::{closure::Closure, JsCast};
            let callback = Closure::wrap(Box::new(move || {
                while sounds.pop().is_some() {}
                while buses.pop().is_some() {}
            }) as Box<dyn FnMut()>);
            let timer = web_sys::window()
                .expect("browser audio requires Window")
                .set_interval_with_callback_and_timeout_and_arguments_0(
                    callback.as_ref().unchecked_ref(),
                    16,
                )
                .expect("audio maintenance timer");
            Self {
                stopped,
                timer,
                _callback: callback,
            }
        }
    }
}
impl Drop for Maintenance {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Release);
        #[cfg(target_arch = "wasm32")]
        if let Some(window) = web_sys::window() {
            window.clear_interval_with_handle(self.timer);
        }
    }
}
