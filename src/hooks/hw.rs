#![allow(non_snake_case)]
#![allow(non_upper_case_globals)]

use error_chain::ChainedError;
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
use std::sync::RwLock;

use capture;
use command;
use cvar;
use dl;
use encode;
use engine::Engine;
use errors::*;
use function::Function;
use sdl;

lazy_static!{
    static ref POINTERS: RwLock<Pointers> = RwLock::new(Pointers::default());
}

#[derive(Debug, Default)]
pub struct Pointers {
    RunListenServer: Function<unsafe extern "C" fn(*mut c_void,
                                                   *mut c_char,
                                                   *mut c_char,
                                                   *mut c_char,
                                                   *mut c_void,
                                                   *mut c_void)
                                                   -> c_int>,

    CL_Disconnect: Function<unsafe extern "C" fn()>,
    Cmd_AddCommand: Function<unsafe extern "C" fn(*const c_char, *mut c_void)>,
    Cmd_Argc: Function<unsafe extern "C" fn() -> c_int>,
    Cmd_Argv: Function<unsafe extern "C" fn(c_int) -> *const c_char>,
    Con_Printf: Function<unsafe extern "C" fn(*const c_char)>,
    Con_ToggleConsole_f: Function<unsafe extern "C" fn()>,
    Cvar_RegisterVariable: Function<unsafe extern "C" fn(*mut cvar::cvar_t)>,
    GL_EndRendering: Function<unsafe extern "C" fn()>,
    Host_FilterTime: Function<unsafe extern "C" fn(c_float) -> c_int>,
    Key_Event: Function<unsafe extern "C" fn(key: c_int, down: c_int)>,
    Memory_Init: Function<unsafe extern "C" fn(*mut c_void, c_int)>,
    S_PaintChannels: Function<unsafe extern "C" fn(endtime: c_int)>,
    S_TransferStereo16: Function<unsafe extern "C" fn(end: c_int)>,
    Sys_VID_FlipScreen: Function<unsafe extern "C" fn()>,
    VideoMode_GetCurrentVideoMode:
        Function<unsafe extern "C" fn(*mut c_int, *mut c_int, *mut c_int)>,
    VideoMode_IsWindowed: Function<unsafe extern "C" fn() -> c_int>,

    cls: Option<*mut client_static_t>,
    host_frametime: Option<*mut c_double>,
    paintbuffer: Option<*mut portable_samplepair_t>, // [1026]
    paintedtime: Option<*mut c_int>,
    realtime: Option<*mut c_double>,
    s_BackBufferFBO: Option<*mut FBO_Container_t>,
    shm: Option<*mut *mut dma_t>,
    window_rect: Option<*mut RECT>,
}

// TODO: think about how to deal with unsafety here.
// The compiler complaining about *mut not being Send/Sync is perfectly valid here.
// Our safe RwLock cannot guarantee there isn't some game thread accessing the values
// at the same time. Although reading/writing to a pointer is already unsafe?
unsafe impl Send for Pointers {}
unsafe impl Sync for Pointers {}

static mut CAPTURE_SOUND: bool = false;
static mut SOUND_REMAINDER: f64 = 0f64;
static mut SOUND_CAPTURE_MODE: SoundCaptureMode = SoundCaptureMode::Normal;
static mut INSIDE_KEY_EVENT: bool = false;
static KERNEL_SRC: &str = r#"
    __kernel void increase_blue(read_only image2d_t src_image,
                                write_only image2d_t dst_image) {
        int2 coord = (int2)(get_global_id(0), get_global_id(1));
        float4 pixel = read_imagef(src_image, coord);
        pixel += (float4)(0.0, 0.0, 0.5, 0.0);
        write_imagef(dst_image, coord, pixel);
    }
"#;

thread_local! {
    static AUDIO_BUFFER: RefCell<Option<capture::AudioBuffer>> = RefCell::new(None);
    static PROQUE: ocl::ProQue = ocl::ProQue::builder()
        .context(ocl::Context::builder()
                 .gl_context(sdl::get_current_context())
                 .glx_display(unsafe { glx::GetCurrentDisplay() } as _)
                 .build()
                 .expect("Context build()"))
        .src(KERNEL_SRC)
        .build()
        .expect("ProQue build()");
}

enum SoundCaptureMode {
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

    // Initialize the encoding.
    encode::initialize();

    // Initialize the capturing.
    capture::initialize();

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
    if capture::is_capturing() && (*POINTERS.read().unwrap().cls.unwrap()).demoplayback != 0 {
        let mut engine = Engine::new();

        if cap_playdemostop.get(&engine).parse(&mut engine).unwrap_or(0) != 0 {
            capture::stop(&engine);
        }
    }

    real!(CL_Disconnect)();
}

/// Handler for the `toggleconsole` command.
#[no_mangle]
pub unsafe extern "C" fn Con_ToggleConsole_f() {
    let mut engine = Engine::new();

    if !INSIDE_KEY_EVENT || cap_allow_tabbing_out_in_demos.get(&engine).parse(&mut engine).unwrap_or(0) == 0 {
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
    let old_realtime = *POINTERS.read().unwrap().realtime.unwrap();

    let rv = real!(Host_FilterTime)(time);

    // TODO: this will NOT set the frametime on the first frame of capture / demo playback and WILL
    // set the frametime on the first frame of not capturing. This needs to be fixed somehow.
    if capture::is_capturing() && (*POINTERS.read().unwrap().cls.unwrap()).demoplayback != 0 {
        if let Some(frametime) = capture::get_frametime() {
            *POINTERS.read().unwrap().host_frametime.unwrap() = frametime;
            *POINTERS.read().unwrap().realtime.unwrap() = old_realtime + frametime;
            return 1;
        }
    }

    rv
}

/// Handles key callbacks.
#[no_mangle]
pub unsafe extern "C" fn Key_Event(key: c_int, down: c_int) {
    INSIDE_KEY_EVENT = true;
    real!(Key_Event)(key, down);
    INSIDE_KEY_EVENT = false;
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
    if !capture::is_capturing() {
        CAPTURE_SOUND = false;
        real!(S_PaintChannels)(endtime);
        return;
    }

    if CAPTURE_SOUND {
        let mut engine = Engine::new();

        let paintedtime = *POINTERS.read().unwrap().paintedtime.unwrap();
        let frametime = match SOUND_CAPTURE_MODE {
            SoundCaptureMode::Normal => *POINTERS.read().unwrap().host_frametime.unwrap(),
            SoundCaptureMode::Remaining => cap_sound_extra.get(&engine).parse(&mut engine).unwrap_or(0f64)
        };
        let speed = (**POINTERS.read().unwrap().shm.unwrap()).speed;
        let samples = frametime * speed as f64 + SOUND_REMAINDER;
        let samples_rounded = match SOUND_CAPTURE_MODE {
            SoundCaptureMode::Normal => samples.floor(),
            SoundCaptureMode::Remaining => samples.ceil()
        };

        SOUND_REMAINDER = samples - samples_rounded;

        AUDIO_BUFFER.with(|b| {
            let mut buf = capture::get_audio_buffer();
            buf.data_mut().clear();
            *b.borrow_mut() = Some(buf);
        });

        real!(S_PaintChannels)(paintedtime + samples_rounded as i32);

        AUDIO_BUFFER.with(|b| capture::capture_audio(b.borrow_mut().take().unwrap()));

        CAPTURE_SOUND = false;
    }
}

/// Transfers the contents of the paintbuffer into the output buffer.
#[no_mangle]
pub unsafe extern "C" fn S_TransferStereo16(end: c_int) {
    if CAPTURE_SOUND {
        // capture::AUDIO_PROFILER.with(|p| p.borrow_mut().as_mut().unwrap().start_section("S_TransferStereo16() overhead"));
        AUDIO_BUFFER.with(|b| {
            let mut buf = b.borrow_mut();
            let mut buf = buf.as_mut().unwrap().data_mut();

            let paintedtime = *POINTERS.read().unwrap().paintedtime.unwrap();
            let paintbuffer = slice::from_raw_parts_mut(POINTERS.read().unwrap().paintbuffer.unwrap(), 1026);

            let mut engine = Engine::new();
            let volume = (cap_volume.get(&engine).parse(&mut engine).unwrap_or(0.4f32) * 256f32) as i32;

            for i in 0..(end - paintedtime) as usize * 2 {
                // Clamping as done in Snd_WriteLinearBlastStereo16().
                let l16 = cmp::min(32767, cmp::max(-32768, (paintbuffer[i].left * volume) >> 8)) as i16;
                let r16 = cmp::min(32767, cmp::max(-32768, (paintbuffer[i].right * volume) >> 8)) as i16;

                buf.push((l16, r16));
            }
        });
        // capture::AUDIO_PROFILER.with(|p| p.borrow_mut().as_mut().unwrap().stop(false).unwrap());
    }

    real!(S_TransferStereo16)(end);
}

/// Flips the screen.
#[export_name = "_Z18Sys_VID_FlipScreenv"]
pub unsafe extern "C" fn Sys_VID_FlipScreen() {
    // Print all messages that happened.
    loop {
        match capture::get_message() {
            Some(msg) => con_print(&msg),
            None => break,
        }
    }

    if capture::is_capturing() {
        capture::GAME_THREAD_PROFILER.with(|p| p.borrow_mut().as_mut().unwrap().start_section("get_resolution()"));
        let (w, h) = get_resolution();

        capture::GAME_THREAD_PROFILER.with(|p| p.borrow_mut().as_mut().unwrap().start_section("get_buffer()"));
        let mut buf = capture::get_buffer((w, h));

        let texture = (*POINTERS.read().unwrap().s_BackBufferFBO.unwrap()).Tex;
        if texture != 0 {
            capture::GAME_THREAD_PROFILER.with(|p| p.borrow_mut().as_mut().unwrap().start_section("gl::Finish()"));
            gl::Finish();

            PROQUE.with(|pro_que| {
                capture::GAME_THREAD_PROFILER.with(|p| p.borrow_mut().as_mut().unwrap().start_section("ImageDescriptor::new()"));
                let descr = ocl::builders::ImageDescriptor::new(ocl::enums::MemObjectType::Image2d,
                                                                w as usize,
                                                                h as usize,
                                                                1,
                                                                1,
                                                                0,
                                                                0,
                                                                None);

                capture::GAME_THREAD_PROFILER.with(|p| p.borrow_mut().as_mut().unwrap().start_section("Image::from_gl_texture()"));
                let image = ocl::Image::<u8>::from_gl_texture(pro_que.queue(),
                                                        ocl::flags::MEM_READ_ONLY,
                                                        descr,
                                                        ocl::core::GlTextureTarget::GlTexture2d,
                                                        0,
                                                        texture)
                    .expect("ocl::Image::from_gl_texture()");

                capture::GAME_THREAD_PROFILER.with(|p| p.borrow_mut().as_mut().unwrap().start_section("Building dst_image"));
                let dst_image = ocl::Image::<u8>::builder()
                    .channel_order(ocl::enums::ImageChannelOrder::Rgba)
                    .channel_data_type(ocl::enums::ImageChannelDataType::UnormInt8)
                    .image_type(ocl::enums::MemObjectType::Image2d)
                    .dims((w, h))
                    .flags(ocl::flags::MEM_WRITE_ONLY | ocl::flags::MEM_HOST_READ_ONLY)
                    .queue(pro_que.queue().clone())
                    .build()
                    .expect("Image build");

                capture::GAME_THREAD_PROFILER.with(|p| p.borrow_mut().as_mut().unwrap().start_section("gl_acquire()"));
                image.cmd().gl_acquire().enq().expect("gl_acquire()");

                capture::GAME_THREAD_PROFILER.with(|p| p.borrow_mut().as_mut().unwrap().start_section("Creating kernel"));
                let kernel = pro_que.create_kernel("increase_blue")
                    .unwrap()
                    .gws((w, h))
                    .arg_img(&image)
                    .arg_img(&dst_image);

                capture::GAME_THREAD_PROFILER.with(|p| p.borrow_mut().as_mut().unwrap().start_section("kernel.enq()"));
                kernel.enq().expect("kernel.enq()");

                capture::GAME_THREAD_PROFILER.with(|p| p.borrow_mut().as_mut().unwrap().start_section("pro_que.finish()"));
                pro_que.finish().expect("pro_que.finish()");

                capture::GAME_THREAD_PROFILER.with(|p| p.borrow_mut().as_mut().unwrap().start_section("dst_image.read()"));
                dst_image.read(buf.as_mut_slice()).enq().expect("dst_image.read()");

                capture::GAME_THREAD_PROFILER.with(|p| p.borrow_mut().as_mut().unwrap().start_section("gl_release()"));
                image.cmd().gl_release().enq().expect("gl_release()");
            });
        } else {
            capture::GAME_THREAD_PROFILER.with(|p| p.borrow_mut().as_mut().unwrap().start_section("gl::PixelStorei()"));
            // Our buffer expects 1-byte alignment.
            gl::PixelStorei(gl::PACK_ALIGNMENT, 1);

            capture::GAME_THREAD_PROFILER.with(|p| p.borrow_mut().as_mut().unwrap().start_section("gl::ReadPixels()"));
            // Get the pixels!
            gl::ReadPixels(0,
                           0,
                           w as GLsizei,
                           h as GLsizei,
                           gl::RGB,
                           gl::UNSIGNED_BYTE,
                           buf.as_mut_slice().as_mut_ptr() as _);
        }

        capture::GAME_THREAD_PROFILER.with(|p| p.borrow_mut().as_mut().unwrap().start_section("capture()"));
        capture::capture(buf, *POINTERS.read().unwrap().host_frametime.unwrap());

        capture::GAME_THREAD_PROFILER.with(|p| p.borrow_mut().as_mut().unwrap().stop(false).unwrap());

        CAPTURE_SOUND = true;
    }

    real!(Sys_VID_FlipScreen)();

    // TODO: check if we're called from SCR_UpdateScreen().
}

/// Obtains and stores all necessary function and variable addresses.
fn refresh_pointers() -> Result<()> {
    let hw = dl::open("hw.so", RTLD_NOW | RTLD_NOLOAD)
        .chain_err(|| "couldn't load hw.so")?;

    let mut pointers = POINTERS.write().unwrap();

    unsafe {
        find!(pointers,
              hw,
              RunListenServer,
              "_Z15RunListenServerPvPcS0_S0_PFP14IBaseInterfacePKcPiES7_");
        find!(pointers, hw, CL_Disconnect, "CL_Disconnect");
        find!(pointers, hw, Cmd_AddCommand, "Cmd_AddCommand");
        find!(pointers, hw, Cmd_Argc, "Cmd_Argc");
        find!(pointers, hw, Cmd_Argv, "Cmd_Argv");
        find!(pointers, hw, Con_Printf, "Con_Printf");
        find!(pointers, hw, Con_ToggleConsole_f, "Con_ToggleConsole_f");
        find!(pointers, hw, Cvar_RegisterVariable, "Cvar_RegisterVariable");
        find!(pointers, hw, GL_EndRendering, "GL_EndRendering");
        find!(pointers, hw, Host_FilterTime, "Host_FilterTime");
        find!(pointers, hw, Key_Event, "Key_Event");
        find!(pointers, hw, Memory_Init, "Memory_Init");
        find!(pointers, hw, S_PaintChannels, "S_PaintChannels");
        find!(pointers, hw, S_TransferStereo16, "S_TransferStereo16");
        find!(pointers, hw, Sys_VID_FlipScreen, "_Z18Sys_VID_FlipScreenv");
        find!(pointers,
              hw,
              VideoMode_GetCurrentVideoMode,
              "VideoMode_GetCurrentVideoMode");
        find!(pointers, hw, VideoMode_IsWindowed, "VideoMode_IsWindowed");

        pointers.cls = Some(hw.sym("cls")
                              .chain_err(|| "couldn't find cls")? as _);
        pointers.host_frametime = Some(hw.sym("host_frametime")
                                         .chain_err(|| "couldn't find host_frametime")? as _);
        pointers.paintbuffer = Some(hw.sym("paintbuffer")
                                      .chain_err(|| "couldn't find paintbuffer")? as _);
        pointers.paintedtime = Some(hw.sym("paintedtime")
                                      .chain_err(|| "couldn't find paintedtime")? as _);
        pointers.realtime = Some(hw.sym("realtime")
                                   .chain_err(|| "couldn't find realtime")? as _);
        pointers.s_BackBufferFBO = Some(hw.sym("s_BackBufferFBO")
                                          .chain_err(|| "couldn't find s_BackBufferFBO")? as _);
        pointers.shm = Some(hw.sym("shm")
                              .chain_err(|| "couldn't find shm")? as _);
        pointers.window_rect = Some(hw.sym("window_rect")
                                      .chain_err(|| "couldn't find window_rect")? as _);
    }

    Ok(())
}

/// Resets all pointers to their default values.
fn reset_pointers() {
    *POINTERS.write().unwrap() = Pointers::default();
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
            cvar.get(&engine)
                .register(&mut engine)
                .chain_err(|| "error registering a console variable") {
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
///
/// # Safety
/// Unsafe because this function should only be called from the main game thread.
pub unsafe fn get_resolution() -> (u32, u32) {
    let mut width;
    let mut height;

    if real!(VideoMode_IsWindowed)() != 0 {
        let window_rect = *POINTERS.read().unwrap().window_rect.unwrap();
        width = window_rect.right - window_rect.left;
        height = window_rect.bottom - window_rect.top;
    } else {
        width = mem::uninitialized();
        height = mem::uninitialized();
        real!(VideoMode_GetCurrentVideoMode)(&mut width, &mut height, ptr::null_mut());
    }

    width = cmp::max(0, width);
    height = cmp::max(0, height);

    (width as u32, height as u32)
}

/// Resets the sound capture remainder.
pub fn reset_sound_capture_remainder(_: &Engine) {
    unsafe { SOUND_REMAINDER = 0f64; }
}

/// Captures the remaining and extra sound.
pub fn capture_remaining_sound(_: &Engine) {
    unsafe {
        SOUND_CAPTURE_MODE = SoundCaptureMode::Remaining;
        CAPTURE_SOUND = true;
        S_PaintChannels(0);
        SOUND_CAPTURE_MODE = SoundCaptureMode::Normal;
    }
}

cvar!(cap_allow_tabbing_out_in_demos, "1");
cvar!(cap_playdemostop, "1");
cvar!(cap_sound_extra, "0");
cvar!(cap_volume, "0.4");
