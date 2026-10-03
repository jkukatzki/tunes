//! Own browser audio activation alongside the stream, independent of the host page.
//!
//! CPAL's `play()` returns before AudioContext.resume() resolves. Safari can leave
//! that promise pending until resume() is called directly inside a user gesture.
//! Calling play() again is unsuitable: CPAL also starts its scheduling timers again.

use super::web_audio_recovery::{ForegroundReturn, Recovery};
use crate::error::{Result, TunesError};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};
use wasm_bindgen::{closure::Closure, JsCast};
use web_sys::{AudioContext, AudioContextState, Event, EventTarget};

struct Listener {
    target: EventTarget,
    name: &'static str,
    callback: Closure<dyn FnMut(Event)>,
}

impl Drop for Listener {
    fn drop(&mut self) {
        let _ = self.target.remove_event_listener_with_callback_and_bool(
            self.name,
            self.callback.as_ref().unchecked_ref(),
            true,
        );
    }
}

pub(super) struct WebAudioLifecycle {
    listeners: Vec<Listener>,
    recovery_timer: Option<(i32, Closure<dyn FnMut()>)>,
    foreground_return: Rc<RefCell<ForegroundReturn>>,
    context: AudioContext,
    document: web_sys::Document,
    pause_when_hidden: Rc<Cell<bool>>,
    hidden: Rc<Cell<bool>>,
}

impl WebAudioLifecycle {
    pub(super) fn new(stream: &cpal::Stream) -> Result<Self> {
        let cpal::platform::StreamInner::WebAudio(stream) = stream.as_inner();
        let context = stream.audio_context();
        let window = web_sys::window().ok_or_else(|| {
            TunesError::AudioEngineError("Web audio requires a browser window".into())
        })?;
        let document = window.document().ok_or_else(|| {
            TunesError::AudioEngineError("Web audio requires a browser document".into())
        })?;
        let pause_when_hidden = Rc::new(Cell::new(false));
        let hidden = Rc::new(Cell::new(document.hidden()));
        let foreground_return = Rc::new(RefCell::new(ForegroundReturn::default()));
        foreground_return.borrow_mut().observe(document.hidden());
        let recovery = Rc::new(RefCell::new(Recovery::default()));
        let report_ticks = Rc::new(Cell::new(2_u8));
        let mut lifecycle = Self {
            listeners: vec![],
            recovery_timer: None,
            foreground_return: foreground_return.clone(),
            context: context.clone(),
            document: document.clone(),
            pause_when_hidden: pause_when_hidden.clone(),
            hidden: hidden.clone(),
        };

        // Capture on window so canvas handlers stopping propagation cannot swallow
        // activation. Keep listeners for the engine's lifetime: iOS can interrupt
        // audio again after a screen lock or app switch.
        for name in ["touchend", "pointerup", "click", "keydown"] {
            let context = context.clone();
            let document = document.clone();
            let pause_when_hidden = pause_when_hidden.clone();
            let hidden = hidden.clone();
            let recovery = recovery.clone();
            lifecycle.listen(window.as_ref(), name, move |event| {
                if event.is_trusted()
                    && !(pause_when_hidden.get() && (hidden.get() || document.hidden()))
                {
                    recovery.borrow_mut().gesture(context.current_time());
                    resume(&context);
                }
            })?;
        }

        let visible_context = context.clone();
        let visible_document = document.clone();
        let visible_policy = pause_when_hidden.clone();
        let visible_hidden = hidden.clone();
        let visible_recovery = recovery.clone();
        let visible_report = report_ticks.clone();
        let visible_return = foreground_return.clone();
        lifecycle.listen(document.as_ref(), "visibilitychange", move |_| {
            visible_hidden.set(visible_document.hidden());
            visible_return
                .borrow_mut()
                .observe(visible_document.hidden());
            if visible_document.hidden() {
                visible_recovery.borrow_mut().cancel();
                if visible_policy.get() {
                    suspend(&visible_context, visible_hidden.clone());
                }
            } else {
                visible_report.set(2);
                visible_recovery
                    .borrow_mut()
                    .arm(visible_context.current_time());
                resume(&visible_context);
            }
        })?;
        let page_context = context.clone();
        let page_document = document.clone();
        let page_policy = pause_when_hidden.clone();
        let page_hidden = hidden.clone();
        let page_recovery = recovery.clone();
        let page_report = report_ticks.clone();
        let page_return = foreground_return.clone();
        lifecycle.listen(window.as_ref(), "pageshow", move |_| {
            page_hidden.set(page_document.hidden());
            page_return.borrow_mut().observe(page_document.hidden());
            if !(page_policy.get() && page_document.hidden()) {
                page_report.set(2);
                page_recovery.borrow_mut().arm(page_context.current_time());
                resume(&page_context);
            }
        })?;
        let hidden_context = context.clone();
        let hidden_policy = pause_when_hidden.clone();
        let hide_state = hidden.clone();
        let hide_recovery = recovery.clone();
        let hide_return = foreground_return.clone();
        lifecycle.listen(window.as_ref(), "pagehide", move |_| {
            hide_state.set(true);
            hide_return.borrow_mut().observe(true);
            hide_recovery.borrow_mut().cancel();
            if hidden_policy.get() {
                suspend(&hidden_context, hide_state.clone());
            }
        })?;

        // Log actual browser state, not just successful stream construction.
        let state_context = context.clone();
        let state_document = document.clone();
        let state_policy = pause_when_hidden.clone();
        let state_hidden = hidden.clone();
        let state_recovery = recovery.clone();
        lifecycle.listen(context.as_ref(), "statechange", move |_| {
            report_state(&state_context);
            // A pending resume can resolve after the page has been hidden.
            if state_policy.get()
                && (state_hidden.get() || state_document.hidden())
                && state_context.state() == AudioContextState::Running
            {
                suspend(&state_context, state_hidden.clone());
            } else if !state_hidden.get()
                && !state_document.hidden()
                && !matches!(
                    state_context.state(),
                    AudioContextState::Running | AudioContextState::Closed
                )
            {
                // Safari can deliver its interrupted/suspended transition after pageshow.
                state_recovery
                    .borrow_mut()
                    .interrupted(state_context.current_time());
            }
        })?;
        let check_context = context.clone();
        let check_document = document.clone();
        let check_hidden = hidden.clone();
        let mut last_clock = context.current_time();
        let check = Closure::wrap(Box::new(move || {
            if check_hidden.get()
                || check_document.hidden()
                || check_context.state() == AudioContextState::Closed
            {
                recovery.borrow_mut().cancel();
                return;
            }
            let clock = check_context.current_time();
            if report_ticks.get() > 0 {
                report_ticks.set(report_ticks.get() - 1);
                if report_ticks.get() == 0 {
                    web_sys::console::info_1(&format!(
                        "[tunes] Foreground audio health: state={:?}, clock_delta={:.3}s",
                        check_context.state(), clock - last_clock,
                    ).into());
                }
            }
            last_clock = clock;
            let running = check_context.state() == AudioContextState::Running;
            if recovery
                .borrow_mut()
                .check(running, check_context.current_time())
            {
                web_sys::console::info_1(
                    &"[tunes] Recovering foreground AudioContext: clock has not resumed".into(),
                );
                if check_context.state() == AudioContextState::Suspended {
                    resume(&check_context);
                } else {
                    // Includes a frozen Running context and Safari's Interrupted state.
                    // suspend() resumes after completion only if we are still visible.
                    suspend(&check_context, check_hidden.clone());
                }
            }
        }) as Box<dyn FnMut()>);
        let timer = window
            .set_interval_with_callback_and_timeout_and_arguments_0(
                check.as_ref().unchecked_ref(),
                500,
            )
            .map_err(|error| {
                TunesError::AudioEngineError(format!(
                    "Failed to monitor web audio recovery: {error:?}"
                ))
            })?;
        lifecycle.recovery_timer = Some((timer, check));
        report_state(context);
        Ok(lifecycle)
    }

    pub(super) fn take_foreground_return(&self) -> bool {
        if !self.pause_when_hidden.get() || self.hidden.get() || self.document.hidden() {
            return false;
        }
        self.foreground_return.borrow_mut().take()
    }

    pub(super) fn set_pause_when_hidden(&self, enabled: bool) {
        self.pause_when_hidden.set(enabled);
        if enabled && (self.hidden.get() || self.document.hidden()) {
            suspend(&self.context, self.hidden.clone());
        }
    }

    fn listen(
        &mut self,
        target: &EventTarget,
        name: &'static str,
        callback: impl FnMut(Event) + 'static,
    ) -> Result<()> {
        let callback = Closure::wrap(Box::new(callback) as Box<dyn FnMut(Event)>);
        target
            .add_event_listener_with_callback_and_bool(
                name,
                callback.as_ref().unchecked_ref(),
                true,
            )
            .map_err(|error| {
                TunesError::AudioEngineError(format!(
                    "Failed to register web audio {name} listener: {error:?}"
                ))
            })?;
        self.listeners.push(Listener {
            target: target.clone(),
            name,
            callback,
        });
        Ok(())
    }
}

fn suspend(context: &AudioContext, hidden: Rc<Cell<bool>>) {
    if matches!(
        context.state(),
        AudioContextState::Suspended | AudioContextState::Closed
    ) {
        return;
    }
    match context.suspend() {
        Ok(promise) => {
            let context = context.clone();
            wasm_bindgen_futures::spawn_local(async move {
                match wasm_bindgen_futures::JsFuture::from(promise).await {
                    Err(error) => web_sys::console::warn_2(
                        &"[tunes] AudioContext suspend rejected".into(),
                        &error,
                    ),
                    // A quick return can happen before suspension finishes.
                    Ok(_) if !hidden.get() => resume(&context),
                    Ok(_) => {}
                }
            });
        }
        Err(error) => {
            web_sys::console::warn_2(&"[tunes] AudioContext suspend failed".into(), &error)
        }
    }
}

fn resume(context: &AudioContext) {
    if matches!(
        context.state(),
        AudioContextState::Running | AudioContextState::Closed
    ) {
        return;
    }
    // Includes Safari's "interrupted" state, even with web-sys versions that do
    // not name that enum variant. Do not defer the resume call to an async task:
    // it must execute on the original DOM user-gesture stack.
    match context.resume() {
        Ok(promise) => wasm_bindgen_futures::spawn_local(async move {
            if let Err(error) = wasm_bindgen_futures::JsFuture::from(promise).await {
                web_sys::console::warn_2(&"[tunes] AudioContext resume rejected".into(), &error);
            }
        }),
        Err(error) => {
            web_sys::console::warn_2(&"[tunes] AudioContext resume failed".into(), &error);
        }
    }
}

fn report_state(context: &AudioContext) {
    web_sys::console::info_1(
        &format!(
            "[tunes] AudioContext state={:?}, sample_rate={} Hz, current_time={:.3}s",
            context.state(),
            context.sample_rate(),
            context.current_time(),
        )
        .into(),
    );
}

impl Drop for WebAudioLifecycle {
    fn drop(&mut self) {
        if let Some((timer, _)) = self.recovery_timer.as_ref() {
            if let Some(window) = web_sys::window() {
                window.clear_interval_with_handle(*timer);
            }
        }
    }
}
