// SPDX-License-Identifier: GPL-3.0-or-later

use std::{sync::Mutex, rc::Rc, cell::RefCell};

use sdl2::{
    event::Event,
    keyboard::Keycode,
    EventPump, VideoSubsystem, render::Canvas, video::Window, pixels,
};

lazy_static::lazy_static! {
    pub static ref SDL: Mutex<SdlEngine> = Mutex::new(SdlEngine::new());
}

pub struct SdlEngine {
    event_pump: EventPump,
    video_subsystem: VideoSubsystem,
}

/// How often we run the poll work in the code hook (SDL pump, ext-device TCP,
/// peripheral deadlines like TIM5's compare match), gated as
/// `n & PUMP_EVENT_INST_INTERVAL == 0` — so this must be a power of two minus
/// one. The previous value of 100_000 was AND-ed as a *mask*, not a modulus:
/// it polled in bursts (any `n` with bits 5/7/9/10/15/16 clear) with dead
/// zones up to ~98k instructions, which put ~450us of firmware-time jitter on
/// every TIM5-scheduled event — rusEFI's trigger emulator teeth wobbled too
/// much for its own decoder to sync on. 1023 polls uniformly every 1024
/// instructions (~4.7us of firmware time at the 216MHz=1instr/cycle
/// convention), and is cheaper on average than the old burst pattern.
pub const PUMP_EVENT_INST_INTERVAL: u64 = 1023;

unsafe impl Send for SdlEngine {}
unsafe impl Sync for SdlEngine {}

impl SdlEngine {
    pub fn new() -> Self {
        let sdl_context = sdl2::init().unwrap();
        let video_subsystem = sdl_context.video().unwrap();

        let event_pump = sdl_context.event_pump().unwrap();

        Self { event_pump, video_subsystem }
    }

    pub fn new_canvas(&mut self, title: &str, width: u32, height: u32) -> Canvas<Window> {
        let window = self.video_subsystem.window(title, width, height)
            .resizable()
            .build()
            .unwrap();

        let mut canvas = window.into_canvas().build().unwrap();

        canvas.set_draw_color(pixels::Color::RGB(0, 0, 0));
        canvas.clear();
        canvas.present();

        canvas
    }

    /// Returns false if we need to quit
    pub fn pump_events(&mut self, framebuffers: &[Rc<RefCell<super::Sdl>>]) -> bool {
        for event in self.event_pump.poll_iter() {
            match event {
                Event::Quit {..} |
                Event::KeyDown { keycode: Some(Keycode::Q), .. } |
                Event::KeyDown { keycode: Some(Keycode::Escape), .. } => {
                    return false;
                },
                Event::MouseMotion { ref window_id, .. } |
                Event::MouseButtonDown { ref window_id, .. } |
                Event::MouseButtonUp { ref window_id, .. } => {
                    if let Some(fb) = framebuffers.iter().find(|fb| fb.borrow().window_id == *window_id) {
                        fb.borrow_mut().process_event(event);
                    }
                }
                _ => {}
            }
        }
        true
    }
}
