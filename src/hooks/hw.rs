#![allow(non_snake_case)]
#![allow(non_upper_case_globals)]

use error_chain::ChainedError;
use ffmpeg::format;
use libc::*;
use gl;
use glx;
use gl::types::*;
use ocl;
use std::cell::RefCell;
use std::cmp;
use std::ffi::{CStr, CString};
use std::mem;
use std::ptr;
use std::slice;

use capture::{self, GameThreadEvent};
use command;
use cvar;
use dl;
use encode;
use engine::Engine;
use errors::*;
use sdl;

// Stuff from these variables should ONLY be accessed from the main game thread.
static mut FUNCTIONS: Option<Functions> = None;
static mut POINTERS: Option<Pointers> = None;

struct Functions {
    RunListenServer: unsafe extern "C" fn(*mut c_void,
                                          *mut c_char,
                                          *mut c_char,
                                          *mut c_char,
                                          *mut c_void,
                                          *mut c_void)
                                          -> c_int,

    CL_Disconnect: unsafe extern "C" fn(),
    Cmd_AddCommand: unsafe extern "C" fn(*const c_char, *mut c_void),
    Cmd_Argc: unsafe extern "C" fn() -> c_int,
    Cmd_Argv: unsafe extern "C" fn(c_int) -> *const c_char,
    Con_Printf: unsafe extern "C" fn(*const c_char),
    Con_ToggleConsole_f: unsafe extern "C" fn(),
    Cvar_RegisterVariable: unsafe extern "C" fn(*mut cvar::cvar_t),
    GL_EndRendering: unsafe extern "C" fn(),
    Host_FilterTime: unsafe extern "C" fn(c_float) -> c_int,
    Key_Event: unsafe extern "C" fn(key: c_int, down: c_int),
    Memory_Init: unsafe extern "C" fn(*mut c_void, c_int),
    S_PaintChannels: unsafe extern "C" fn(endtime: c_int),
    S_TransferStereo16: unsafe extern "C" fn(end: c_int),
    Sys_VID_FlipScreen: unsafe extern "C" fn(),
    VideoMode_GetCurrentVideoMode: unsafe extern "C" fn(*mut c_int, *mut c_int, *mut c_int),
    VideoMode_IsWindowed: unsafe extern "C" fn() -> c_int,
}

struct Pointers {
    cls: *mut client_static_t,
    host_frametime: *mut c_double,
    paintbuffer: *mut portable_samplepair_t, // [1026]
    paintedtime: *mut c_int,
    realtime: *mut c_double,
    s_BackBufferFBO: *mut FBO_Container_t,
    shm: *mut *mut dma_t,
    window_rect: *mut RECT,
}

thread_local! {
    static AUDIO_BUFFER: RefCell<Option<capture::AudioBuffer>> = RefCell::new(None);
    static PROQUE: ocl::ProQue = ocl::ProQue::builder()
        .context(ocl::Context::builder()
                 .gl_context(sdl::get_current_context())
                 .glx_display(unsafe { glx::GetCurrentDisplay() } as _)
                 .build()
                 .expect("Context build()"))
        .src(include_str!("../../cl_src/color_conversion.cl"))
        .build()
        .expect("ProQue build()");
}

pub enum SoundCaptureMode {
    Normal,
    Remaining,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct RECT {
    left: c_int,
    right: c_int,
    top: c_int,
    bottom: c_int,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct FBO_Container_t {
    FBO: GLuint,
    CB: GLuint,
    DB: GLuint,
    Tex: GLuint,
}

#[repr(C)]
struct client_static_t {
    stuff: [u8; 0x4060],
    demoplayback: c_int,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct portable_samplepair_t {
    left: c_int,
    right: c_int,
}

#[repr(C)]
struct dma_t {
    gamealive: c_int,
    soundalive: c_int,
    splitbuffer: c_int,
    channels: c_int,
    samples: c_int,
    submission_chunk: c_int,
    samplepos: c_int,
    samplebits: c_int,
    speed: c_int,
    dmaspeed: c_int,
    buffer: *mut c_uchar,
}

/// The "main" function of hw.so, called inside `CEngineAPI::Run()`.
///
/// The game runs within this function and shortly after it exits hw.so is unloaded.
/// Note: `_restart` also causes this function to exit, in this case the launcher
/// unloads and reloads hw.so and this function is called again as if it was a fresh start.
#[export_name = "_Z15RunListenServerPvPcS0_S0_PFP14IBaseInterfacePKcPiES7_"]
pub unsafe extern "C" fn RunListenServer(instance: *mut c_void,
                                         basedir: *mut c_char,
                                         cmdline: *mut c_char,
                                         postRestartCmdLineArgs: *mut c_char,
                                         launcherFactory: *mut c_void,
                                         filesystemFactory: *mut c_void)
                                         -> c_int {
    // hw.so just loaded, either for the first time or potentially at a different address.
    // Refresh all pointers.
    if let Err(ref e) = refresh_pointers().chain_err(|| "error refreshing pointers") {
        panic!("{}", e.display());
    }

    let engine = Engine::new();

    // Initialize the encoding.
    encode::initialize();

    // Initialize the capturing.
    capture::initialize(&engine);

    let rv = real!(RunListenServer)(instance,
                                    basedir,
                                    cmdline,
                                    postRestartCmdLineArgs,
                                    launcherFactory,
                                    filesystemFactory);

    // Since hw.so is getting unloaded, reset all pointers.
    reset_pointers();

    rv
}

/// Stops the currently running game, returning to the main menu.
#[no_mangle]
pub unsafe extern "C" fn CL_Disconnect() {
    if capture::is_capturing() && (*ptr!(cls)).demoplayback != 0 {
        let mut engine = Engine::new();

        if cap_playdemostop.parse(&mut engine).unwrap_or(0) != 0 {
            capture::stop(&engine);
        }
    }

    real!(CL_Disconnect)();
}

/// Handler for the `toggleconsole` command.
#[no_mangle]
pub unsafe extern "C" fn Con_ToggleConsole_f() {
    let mut engine = Engine::new();

    if !engine.data().inside_key_event ||
        cap_allow_tabbing_out_in_demos.parse(&mut engine)
                                      .unwrap_or(0) == 0
    {
        real!(Con_ToggleConsole_f)();
    }
}

/// Blits the FBOs onto the backbuffer and flips.
#[no_mangle]
pub unsafe extern "C" fn GL_EndRendering() {
    real!(GL_EndRendering)();
}

/// Calculates the frame time and limits the FPS.
#[no_mangle]
pub unsafe extern "C" fn Host_FilterTime(time: c_float) -> c_int {
    let engine = Engine::new();

    let old_realtime = *ptr!(realtime);

    let rv = real!(Host_FilterTime)(time);

    // TODO: this will NOT set the frametime on the first frame of capture / demo playback and WILL
    // set the frametime on the first frame of not capturing. This needs to be fixed somehow.
    if capture::is_capturing() && (*ptr!(cls)).demoplayback != 0 {
        let frametime = capture::get_capture_parameters(&engine).time_base.into();

        *ptr!(host_frametime) = frametime;
        *ptr!(realtime) = old_realtime + frametime;
        return 1;
    }

    rv
}

/// Handles key callbacks.
#[no_mangle]
pub unsafe extern "C" fn Key_Event(key: c_int, down: c_int) {
    let engine = Engine::new();
    engine.data().inside_key_event = true;
    real!(Key_Event)(key, down);
    engine.data().inside_key_event = false;
}

/// Initializes the hunk memory.
///
/// After the hunk memory has been initialized we can register console commands and variables.
/// It's also a good place to get OpenGL function pointers.
#[no_mangle]
pub unsafe extern "C" fn Memory_Init(buf: *mut c_void, size: c_int) {
    real!(Memory_Init)(buf, size);

    register_cvars_and_commands();

    gl::ReadPixels::load_with(|s| sdl::get_proc_address(s) as _);
    if !gl::ReadPixels::is_loaded() {
        panic!("could not load glReadPixels()");
    }
    gl::PixelStorei::load_with(|s| sdl::get_proc_address(s) as _);
    if !gl::PixelStorei::is_loaded() {
        panic!("could not load glPixelStorei()");
    }
    gl::Finish::load_with(|s| sdl::get_proc_address(s) as _);
    if !gl::Finish::is_loaded() {
        panic!("could not load glFinish()");
    }
}

/// Mixes sound into the output buffer using the paintbuffer.
#[no_mangle]
pub unsafe extern "C" fn S_PaintChannels(endtime: c_int) {
    let engine = Engine::new();

    if !capture::is_capturing() {
        engine.data().capture_sound = false;
        real!(S_PaintChannels)(endtime);
        return;
    }

    if engine.data().capture_sound {
        let paintedtime = *ptr!(paintedtime);
        let frametime = match engine.data().sound_capture_mode {
            SoundCaptureMode::Normal => *ptr!(host_frametime),
            SoundCaptureMode::Remaining => capture::get_capture_parameters(&engine).sound_extra,
        };
        let speed = (**ptr!(shm)).speed;
        let samples = frametime * speed as f64 + engine.data().sound_remainder;
        let samples_rounded = match engine.data().sound_capture_mode {
            SoundCaptureMode::Normal => samples.floor(),
            SoundCaptureMode::Remaining => samples.ceil(),
        };

        engine.data().sound_remainder = samples - samples_rounded;

        AUDIO_BUFFER.with(|b| {
                              let mut buf = capture::get_audio_buffer(&engine);
                              buf.data_mut().clear();
                              *b.borrow_mut() = Some(buf);
                          });

        real!(S_PaintChannels)(paintedtime + samples_rounded as i32);

        AUDIO_BUFFER.with(|b| capture::capture_audio(&engine, b.borrow_mut().take().unwrap()));

        engine.data().capture_sound = false;
    }
}

/// Transfers the contents of the paintbuffer into the output buffer.
#[no_mangle]
pub unsafe extern "C" fn S_TransferStereo16(end: c_int) {
    let engine = Engine::new();
    if engine.data().capture_sound {
        AUDIO_BUFFER.with(|b| {
            let mut buf = b.borrow_mut();
            let mut buf = buf.as_mut().unwrap().data_mut();

            let paintedtime = *ptr!(paintedtime);
            let paintbuffer = slice::from_raw_parts_mut(ptr!(paintbuffer), 1026);

            let engine = Engine::new();
            let volume = (capture::get_capture_parameters(&engine).volume * 256f32) as i32;

            for i in 0..(end - paintedtime) as usize * 2 {
                // Clamping as done in Snd_WriteLinearBlastStereo16().
                let l16 = cmp::min(32767, cmp::max(-32768, (paintbuffer[i].left * volume) >> 8)) as
                    i16;
                let r16 = cmp::min(32767,
                                   cmp::max(-32768, (paintbuffer[i].right * volume) >> 8)) as
                    i16;

                buf.push((l16, r16));
            }
        });
    }

    real!(S_TransferStereo16)(end);
}

/// Flips the screen.
#[export_name = "_Z18Sys_VID_FlipScreenv"]
pub unsafe extern "C" fn Sys_VID_FlipScreen() {
    let engine = Engine::new();

    // Print all messages that happened.
    loop {
        match capture::get_event(&engine) {
            Some(e) => {
                match e {
                    GameThreadEvent::Message(msg) => con_print(&msg),
                    GameThreadEvent::EncoderPixelFormat(fmt) => {
                        engine.data().encoder_pixel_format = Some(fmt)
                    }
                }
            }
            None => break,
        }
    }

    // If the encoding just started, wait for the pixel format.
    while capture::is_capturing() && engine.data().encoder_pixel_format.is_none() {
        match capture::get_event_block(&engine) {
            GameThreadEvent::Message(msg) => con_print(&msg),
            GameThreadEvent::EncoderPixelFormat(fmt) => {
                engine.data().encoder_pixel_format = Some(fmt)
            }
        }
    }

    if capture::is_capturing() {
        // Always capture sound.
        engine.data().capture_sound = true;

        let frames = engine.data()
                           .time_interpolator
                           .as_mut()
                           .unwrap()
                           .time_passed(*ptr!(host_frametime));

        // If frames is zero, we need to skip this frame.
        if frames > 0 {
            let (w, h) = get_resolution(&engine);

            let mut buf = capture::get_buffer(&engine, (w, h));

            let texture = (*ptr!(s_BackBufferFBO)).Tex;
            if texture != 0 {
                gl::Finish();

                PROQUE.with(|pro_que| {
                    let descr =
                        ocl::builders::ImageDescriptor::new(ocl::enums::MemObjectType::Image2d,
                                                            w as usize,
                                                            h as usize,
                                                            1,
                                                            1,
                                                            0,
                                                            0,
                                                            None);

                    let image =
                        ocl::Image::<u8>::from_gl_texture(pro_que.queue(),
                                                          ocl::flags::MEM_READ_ONLY,
                                                          descr,
                                                          ocl::core::GlTextureTarget::GlTexture2d,
                                                          0,
                                                          texture)
                        .expect("ocl::Image::from_gl_texture()");

                    image.cmd().gl_acquire().enq().expect("gl_acquire()");

                    if engine.data().encoder_pixel_format.unwrap() == format::Pixel::YUV420P {
                        // TODO
                        buf.set_format(format::Pixel::YUV444P);
                        let mut frame = buf.get_frame();

                        let Y_buf = ocl::Buffer::<u8>::builder()
                            .queue(pro_que.queue().clone())
                            .flags(ocl::flags::MemFlags::new().write_only().host_read_only())
                            .dims(frame.data(0).len())
                            .build()
                            .expect("Buffer build");
                        let U_buf = ocl::Buffer::<u8>::builder()
                            .queue(pro_que.queue().clone())
                            .flags(ocl::flags::MemFlags::new().write_only().host_read_only())
                            .dims(frame.data(1).len())
                            .build()
                            .expect("Buffer build");
                        let V_buf = ocl::Buffer::<u8>::builder()
                            .queue(pro_que.queue().clone())
                            .flags(ocl::flags::MemFlags::new().write_only().host_read_only())
                            .dims(frame.data(2).len())
                            .build()
                            .expect("Buffer build");

                        let kernel = pro_que.create_kernel("rgb_to_yuv444_601_limited")
                                            .unwrap()
                                            .gws((w, h))
                                            .arg_img(&image)
                                            .arg_scl(frame.stride(0))
                                            .arg_scl(frame.stride(1))
                                            .arg_scl(frame.stride(2))
                                            .arg_buf(&Y_buf)
                                            .arg_buf(&U_buf)
                                            .arg_buf(&V_buf);

                        kernel.enq().expect("kernel.enq()");

                        // pro_que.finish().expect("pro_que.finish()");

                        Y_buf.read(frame.data_mut(0)).enq().expect("Y_buf.read()");
                        U_buf.read(frame.data_mut(1)).enq().expect("U_buf.read()");
                        V_buf.read(frame.data_mut(2)).enq().expect("V_buf.read()");
                    } else {
                        buf.set_format(format::Pixel::RGBA);

                        image.read(buf.as_mut_slice()).enq().expect("image.read()");
                    }

                    image.cmd().gl_release().enq().expect("gl_release()");
                });
            } else {
                buf.set_format(format::Pixel::RGB24);

                // Our buffer expects 1-byte alignment.
                gl::PixelStorei(gl::PACK_ALIGNMENT, 1);

                // Get the pixels!
                gl::ReadPixels(0,
                               0,
                               w as GLsizei,
                               h as GLsizei,
                               gl::RGB,
                               gl::UNSIGNED_BYTE,
                               buf.as_mut_slice().as_mut_ptr() as _);
            }

            capture::capture(&engine, buf, frames);
        }
    }

    real!(Sys_VID_FlipScreen)();

    // TODO: check if we're called from SCR_UpdateScreen().
}

/// Obtains and stores all necessary function and variable addresses.
///
/// # Safety
/// Unsafe because this function should only be called from the main game thread.
unsafe fn refresh_pointers() -> Result<()> {
    let hw = dl::open("hw.so", RTLD_NOW | RTLD_NOLOAD)
        .chain_err(|| "couldn't load hw.so")?;

    FUNCTIONS = Some(Functions {
                         RunListenServer: find!(hw,
                                                "_Z15RunListenServerPvPcS0_S0_PFP14IBaseInterfacePKcPiES7_"),
                         CL_Disconnect: find!(hw, "CL_Disconnect"),
                         Cmd_AddCommand: find!(hw, "Cmd_AddCommand"),
                         Cmd_Argc: find!(hw, "Cmd_Argc"),
                         Cmd_Argv: find!(hw, "Cmd_Argv"),
                         Con_Printf: find!(hw, "Con_Printf"),
                         Con_ToggleConsole_f: find!(hw, "Con_ToggleConsole_f"),
                         Cvar_RegisterVariable: find!(hw, "Cvar_RegisterVariable"),
                         GL_EndRendering: find!(hw, "GL_EndRendering"),
                         Host_FilterTime: find!(hw, "Host_FilterTime"),
                         Key_Event: find!(hw, "Key_Event"),
                         Memory_Init: find!(hw, "Memory_Init"),
                         S_PaintChannels: find!(hw, "S_PaintChannels"),
                         S_TransferStereo16: find!(hw, "S_TransferStereo16"),
                         Sys_VID_FlipScreen: find!(hw, "_Z18Sys_VID_FlipScreenv"),
                         VideoMode_GetCurrentVideoMode: find!(hw, "VideoMode_GetCurrentVideoMode"),
                         VideoMode_IsWindowed: find!(hw, "VideoMode_IsWindowed"),
                     });

    POINTERS = Some(Pointers {
                        cls: find!(hw, "cls"),
                        host_frametime: find!(hw, "host_frametime"),
                        paintbuffer: find!(hw, "paintbuffer"),
                        paintedtime: find!(hw, "paintedtime"),
                        realtime: find!(hw, "realtime"),
                        s_BackBufferFBO: find!(hw, "s_BackBufferFBO"),
                        shm: find!(hw, "shm"),
                        window_rect: find!(hw, "window_rect"),
                    });

    Ok(())
}

/// Resets all pointers to their default values.
///
/// # Safety
/// Unsafe because this function should only be called from the main game thread.
unsafe fn reset_pointers() {
    FUNCTIONS = None;
    POINTERS = None;
}

/// Registers console commands and variables.
///
/// # Safety
/// Unsafe because this function should only be called from the main game thread.
unsafe fn register_cvars_and_commands() {
    for cmd in &command::COMMANDS {
        register_command(cmd.name(), cmd.callback());
    }

    let mut engine = Engine::new();
    for cvar in &cvar::CVARS {
        if let Err(ref e) =
            cvar.register(&mut engine)
                .chain_err(|| "error registering a console variable")
        {
            panic!("{}", e.display());
        }
    }
}

/// Registers a console command.
///
/// # Safety
/// Unsafe because this function should only be called from the main game thread.
unsafe fn register_command(name: &'static [u8], callback: unsafe extern "C" fn()) {
    real!(Cmd_AddCommand)(name as *const _ as *const _, callback as *mut c_void);
}

/// Registers a console variable.
///
/// # Safety
/// Unsafe because this function should only be called from the main game thread.
pub unsafe fn register_variable(cvar: &mut cvar::cvar_t) {
    real!(Cvar_RegisterVariable)(cvar);
}

/// Prints the given string to the game console.
///
/// `string` must not contain null bytes.
///
/// # Safety
/// Unsafe because this function should only be called from the main game thread.
pub unsafe fn con_print(string: &str) {
    real!(Con_Printf)(CString::new(string.replace('%', "%%"))
                          .expect("string cannot contain null bytes")
                          .as_ptr())
}

/// Gets the console command argument count.
///
/// # Safety
/// Unsafe because this function should only be called from the main game thread.
pub unsafe fn cmd_argc() -> u32 {
    let argc = real!(Cmd_Argc)();
    cmp::max(0, argc) as u32
}

/// Gets a console command argument.
///
/// # Safety
/// Unsafe because this function should only be called from the main game thread.
pub unsafe fn cmd_argv(index: u32) -> String {
    let index = cmp::min(index, i32::max_value() as u32) as i32;
    let arg = real!(Cmd_Argv)(index);
    CStr::from_ptr(arg).to_string_lossy().into_owned()
}

/// Returns the current game resolution.
pub fn get_resolution(_: &Engine) -> (u32, u32) {
    let mut width;
    let mut height;

    unsafe {
        if real!(VideoMode_IsWindowed)() != 0 {
            let window_rect = *ptr!(window_rect);
            width = window_rect.right - window_rect.left;
            height = window_rect.bottom - window_rect.top;
        } else {
            width = mem::uninitialized();
            height = mem::uninitialized();
            real!(VideoMode_GetCurrentVideoMode)(&mut width, &mut height, ptr::null_mut());
        }
    }

    width = cmp::max(0, width);
    height = cmp::max(0, height);

    (width as u32, height as u32)
}

/// Resets the sound capture remainder.
pub fn reset_sound_capture_remainder(engine: &Engine) {
    engine.data().sound_remainder = 0f64;
}

/// Captures the remaining and extra sound.
pub fn capture_remaining_sound(engine: &Engine) {
    engine.data().sound_capture_mode = SoundCaptureMode::Remaining;
    engine.data().capture_sound = true;
    unsafe {
        S_PaintChannels(0);
    }
    engine.data().sound_capture_mode = SoundCaptureMode::Normal;
}

cvar!(cap_allow_tabbing_out_in_demos, "1");
cvar!(cap_playdemostop, "1");
