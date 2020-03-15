use screeps::*;
use serde::*;
use std::borrow::Cow;

pub type StrCow = Cow<'static, str>;

static mut TRACE: Option<Trace> = None;

pub fn start_trace() {
    unsafe {
        if TRACE.is_some() {
            panic!("Expected trace to be not be set!");
        }

        TRACE = Some(Trace::new());
    }
}

pub fn stop_trace() -> Trace {
    unsafe {
        TRACE.take().unwrap()
    }
}

pub fn get_mut_trace() -> Option<&'static mut Trace> {
    unsafe {
        TRACE.as_mut()
    }
}

#[derive(Debug, Serialize)]
pub struct Trace {
    #[serde(rename = "traceEvents")]
    events: Vec<Event>,
}

#[derive(Serialize)]
struct TracingEvent {
    #[serde(rename = "name")]
    name: StrCow,
    #[serde(rename = "pid")]
    process_id: u32,
    #[serde(rename = "tid")]
    thread_id: u32, 
    #[serde(rename = "ts")]
    timestamp: f64
}

#[derive(Clone, Debug, Serialize)]
#[serde(into = "TracingEvent")]
struct BeginEvent {
    name: StrCow,
    time: f64,
}

impl Into<TracingEvent> for BeginEvent {
    fn into(self) -> TracingEvent {
        TracingEvent {
            name: self.name,
            process_id: 0,
            thread_id: 0,
            timestamp: self.time
        }
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(into = "TracingEvent")]
struct EndEvent {
    name: StrCow,
    time: f64
}

impl Into<TracingEvent> for EndEvent {
    fn into(self) -> TracingEvent {
        TracingEvent {
            name: self.name,
            process_id: 0,
            thread_id: 0,
            timestamp: self.time
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(tag = "ph")]
enum Event {
    #[serde(rename = "B")]
    Begin(BeginEvent),
    #[serde(rename = "E")]
    End(EndEvent)
}

#[must_use = "The guard is immediately dropped after instantiation. This is probably not
what you want! Consider using a `let` binding to increase its lifetime."]
pub struct SpanGuard {
    name: StrCow
}

impl Drop for SpanGuard {
    fn drop(&mut self) {
        end(self.name.clone());
    }
}

pub fn start_guard<S: Into<StrCow>>(name: S) -> SpanGuard {
    let name = name.into();
    start(name.clone());
    SpanGuard { name }
}

fn start<S: Into<StrCow>>(name: S) {
    if let Some(trace) = get_mut_trace() {
        let event = BeginEvent {
            name: name.into(),
            time: game::cpu::get_used(),
        };

        trace.events.push(Event::Begin(event));
    }
}

fn end<S: Into<StrCow>>(name: S) {
    let name = name.into();

    if let Some(trace) = get_mut_trace() {
        let event = EndEvent {
            name: name.into(),
            time: game::cpu::get_used(),
        };

        trace.events.push(Event::End(event));
    }
}

impl Trace {
    fn new() -> Trace {
        Trace {
            events: Vec::new()
        }
    }
}