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
use fps_converter::*;
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
    GL_SetMode: unsafe extern "C" fn(c_int,
                                     *mut c_void,
                                     *mut c_void,
                                     c_int,
                                     *const c_char,
                                     *const c_char)
                                     -> c_int,
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
}

pub enum SoundCaptureMode {
    Normal,
    Remaining,
}

pub struct OclGlTexture {
    image: ocl::Image<u8>,
}

pub enum FrameCapture {
    OpenGL,
    OpenCL(OclGlTexture),
}

impl OclGlTexture {
    fn new(_: &Engine, texture: GLuint, queue: ocl::Queue, dims: ocl::SpatialDims) -> Self {
        unsafe {
            gl::Finish();
        }

        let descr = ocl::builders::ImageDescriptor::new(ocl::enums::MemObjectType::Image2d,
                                                        dims[0],
                                                        dims[1],
                                                        1,
                                                        1,
                                                        0,
                                                        0,
                                                        None);

        let image = ocl::Image::<u8>::from_gl_texture(queue,
                                                      ocl::flags::MEM_READ_ONLY,
                                                      descr,
                                                      ocl::core::GlTextureTarget::GlTexture2d,
                                                      0,
                                                      texture)
                    .expect("ocl::Image::from_gl_texture()");

        image.cmd().gl_acquire().enq().expect("gl_acquire()");

        Self { image }
    }
}

impl AsRef<ocl::Image<u8>> for OclGlTexture {
    fn as_ref(&self) -> &ocl::Image<u8> {
        &self.image
    }
}

impl Drop for OclGlTexture {
    fn drop(&mut self) {
        let mut event = ocl::Event::empty();

        self.image
            .cmd()
            .gl_release()
            .enew(&mut event)
            .enq()
            .expect("gl_release()");

        event.wait_for()
             .expect("waiting for the gl_release() event");
    }
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

    if let Some(Some(buffers)) = engine.data().ocl_yuv_buffers.take() {
        drop(Box::from_raw(buffers));
    }

    if let Some(Some(pro_que)) = engine.data().pro_que.take() {
        drop(Box::from_raw(pro_que));
    }

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

/// Sets up the main FBOs and the display mode.
#[no_mangle]
pub unsafe extern "C" fn GL_SetMode(mainwindow: c_int,
                                    pmaindc: *mut c_void,
                                    pbaseRC: *mut c_void,
                                    fD3D: c_int,
                                    pszDriver: *const c_char,
                                    pszCmdLine: *const c_char)
                                    -> c_int {
    let engine = Engine::new();
    engine.data().inside_gl_setmode = true;

    let rv = real!(GL_SetMode)(mainwindow, pmaindc, pbaseRC, fD3D, pszDriver, pszCmdLine);

    engine.data().inside_gl_setmode = false;

    rv
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
        let params = capture::get_capture_parameters(&engine);
        let frametime = params.sampling_time_base.unwrap_or(params.time_base).into();

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

        match engine.data().fps_converter.as_mut().unwrap() {
            &mut FPSConverters::Simple(ref mut simple_conv) => {
                simple_conv.time_passed(&engine, *ptr!(host_frametime), capture_frame);
            }

            &mut FPSConverters::Sampling(ref mut sampling_conv) => {
                sampling_conv.time_passed(&engine, *ptr!(host_frametime), capture_frame);
            }
        }
    }

    real!(Sys_VID_FlipScreen)();

    // TODO: check if we're called from SCR_UpdateScreen().
}

/// Returns whether the game is running in windowed mode.
#[no_mangle]
pub unsafe fn VideoMode_IsWindowed() -> c_int {
    let engine = Engine::new();

    // Force FBO usage.
    if engine.data().inside_gl_setmode {
        return 0;
    }

    real!(VideoMode_IsWindowed)()
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
                         GL_SetMode: find!(hw, "GL_SetMode"),
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
    let cstring = CString::new(string.replace('%', "%%"))
        .expect("string cannot contain null bytes");
    real!(Con_Printf)(cstring.as_ptr())
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

/// Returns the ocl `ProCue`.
pub fn get_pro_que(engine: &Engine) -> Option<&mut ocl::ProQue> {
    if engine.data().pro_que.is_none() {
        let report_opencl_error = |ref e: Error| {
            engine.con_print(&format!("Could not initialize OpenCL, proceeding without it. \
                                           Error details:\n{}",
                                      e.display())
                              .replace('\0', "\\x00"));
        };

        let context = ocl::Context::builder()
            .gl_context(sdl::get_current_context())
            .glx_display(unsafe { glx::GetCurrentDisplay() } as _)
            .build()
            .chain_err(|| "error building ocl::Context");

        let pro_que = context.and_then(|ctx| {
            ocl::ProQue::builder()
                .context(ctx)
                .prog_bldr(ocl::Program::builder()
                               .src(include_str!("../../cl_src/color_conversion.cl"))
                               .src(include_str!("../../cl_src/sampling.cl")))
                .build()
                .chain_err(|| "error building ocl::ProQue")
        })
                             .map(|pro_que| Box::into_raw(Box::new(pro_que)))
                             .map_err(report_opencl_error)
                             .ok();

        engine.data().pro_que = Some(pro_que);
    }

    engine.data()
          .pro_que
          .as_mut()
          .unwrap()
          .as_mut()
          .map(|ptr| unsafe { ptr.as_mut() }.unwrap())
}

/// Builds an ocl `Buffer` with the specified length.
fn build_ocl_buffer(engine: &Engine,
                    pro_que: &ocl::ProQue,
                    length: usize)
                    -> Option<ocl::Buffer<u8>> {
    ocl::Buffer::<u8>::builder()
        .queue(pro_que.queue().clone())
        .flags(ocl::flags::MemFlags::new().write_only().host_read_only())
        .dims(length)
        .build()
        .chain_err(|| "could not build the OpenCL buffer")
        .map_err(|ref e| { engine.con_print(&format!("{}", e.display())); })
        .ok()
}

/// Builds an ocl `Image` with the specified dimensions.
pub fn build_ocl_image(engine: &Engine,
                       pro_que: &ocl::ProQue,
                       mem_flags: ocl::MemFlags,
                       data_type: ocl::enums::ImageChannelDataType,
                       dims: ocl::SpatialDims)
                       -> Option<ocl::Image<u8>> {
    ocl::Image::<u8>::builder()
        .channel_order(ocl::enums::ImageChannelOrder::Rgba)
        .channel_data_type(data_type)
        .image_type(ocl::enums::MemObjectType::Image2d)
        .dims(dims)
        .flags(mem_flags)
        .queue(pro_que.queue().clone())
        .build()
        .chain_err(|| "could not build the OpenCL image")
        .map_err(|ref e| { engine.con_print(&format!("{}", e.display())); })
        .ok()
}

/// Builds ocl YUV buffers with the specified length.
fn build_yuv_buffers(engine: &Engine,
                     pro_que: &ocl::ProQue,
                     (Y_len, U_len, V_len): (usize, usize, usize))
                     -> Option<*mut (ocl::Buffer<u8>, ocl::Buffer<u8>, ocl::Buffer<u8>)> {
    let Y_buf = build_ocl_buffer(engine, pro_que, Y_len);
    let U_buf = build_ocl_buffer(engine, pro_que, U_len);
    let V_buf = build_ocl_buffer(engine, pro_que, V_len);

    if let (Some(Y_buf), Some(U_buf), Some(V_buf)) = (Y_buf, U_buf, V_buf) {
        Some(Box::into_raw(Box::new((Y_buf, U_buf, V_buf))))
    } else {
        None
    }
}

/// Returns the ocl buffers for Y, U and V.
fn get_yuv_buffers<'a>(engine: &Engine,
                       pro_que: &'a ocl::ProQue,
                       (Y_len, U_len, V_len): (usize, usize, usize))
                       -> Option<&'a mut (ocl::Buffer<u8>, ocl::Buffer<u8>, ocl::Buffer<u8>)> {
    if engine.data().ocl_yuv_buffers.is_none() {
        let buffers = build_yuv_buffers(engine, pro_que, (Y_len, U_len, V_len));
        engine.data().ocl_yuv_buffers = Some(buffers);
    }

    // Verify the buffer sizes.
    match engine.data()
                  .ocl_yuv_buffers
                  .as_mut()
                  .unwrap()
                  .as_mut()
                  .map(|ptr| unsafe { ptr.as_mut() }.unwrap()) {
        Some(&mut (ref Y_buf, ref U_buf, ref V_buf)) => {
            // Check if the requested buffer size is different.
            // In most cases if one of the buffer sizes changes, the other do as well.
            if Y_buf.len() != Y_len || U_buf.len() != U_len || V_buf.len() != V_len {
                // Drop the references first.
                drop(Y_buf);
                drop(U_buf);
                drop(V_buf);

                // Now drop the buffers themselves.
                drop(unsafe {
                         Box::from_raw(engine.data().ocl_yuv_buffers.take().unwrap().unwrap())
                     });

                // And allocate new ones.
                let buffers = build_yuv_buffers(engine, pro_que, (Y_len, U_len, V_len));
                engine.data().ocl_yuv_buffers = Some(buffers);

                buffers.map(|ptr| unsafe { ptr.as_mut() }.unwrap())
            } else {
                // Drop the existing references.
                drop(Y_buf);
                drop(U_buf);
                drop(V_buf);

                engine.data()
                      .ocl_yuv_buffers
                      .as_mut()
                      .unwrap()
                      .as_mut()
                      .map(|ptr| unsafe { ptr.as_mut() }.unwrap())
            }
        }
        None => None,
    }
}

/// Captures and returns the current frame.
fn capture_frame(engine: &Engine) -> FrameCapture {
    let texture = unsafe { *ptr!(s_BackBufferFBO) }.Tex;
    let pro_que = if texture != 0 {
        get_pro_que(&engine)
    } else {
        None
    };

    if let Some(pro_que) = pro_que {
        let (w, h) = get_resolution(&engine);

        FrameCapture::OpenCL(OclGlTexture::new(&engine,
                                               texture,
                                               pro_que.queue().clone(),
                                               (w, h).into()))
    } else {
        FrameCapture::OpenGL
    }
}

/// Reads the given `ocl::Image` into the buffer.
pub fn read_ocl_image_into_buf(engine: &Engine,
                               image: &ocl::Image<u8>,
                               buf: &mut capture::VideoBuffer) {
    let pro_que = get_pro_que(engine).unwrap();

    let yuv_buffers = if engine.data().encoder_pixel_format.unwrap() == format::Pixel::YUV420P {
        buf.set_format(format::Pixel::YUV420P);
        let frame = buf.get_frame();

        get_yuv_buffers(&engine,
                        pro_que,
                        (frame.data(0).len(), frame.data(1).len(), frame.data(2).len()))
    } else {
        None
    };

    if yuv_buffers.is_some() {
        let mut frame = buf.get_frame();

        let &mut (ref Y_buf, ref U_buf, ref V_buf) = yuv_buffers.unwrap();

        let kernel = pro_que.create_kernel("rgb_to_yuv420_601_limited")
                            .unwrap()
                            .gws(image.dims())
                            .arg_img(image)
                            .arg_scl(frame.stride(0))
                            .arg_scl(frame.stride(1))
                            .arg_scl(frame.stride(2))
                            .arg_buf(Y_buf)
                            .arg_buf(U_buf)
                            .arg_buf(V_buf);

        kernel.enq().expect("kernel.enq()");

        Y_buf.read(frame.data_mut(0)).enq().expect("Y_buf.read()");
        U_buf.read(frame.data_mut(1)).enq().expect("U_buf.read()");
        V_buf.read(frame.data_mut(2)).enq().expect("V_buf.read()");
    } else {
        buf.set_format(format::Pixel::RGBA);

        image.read(buf.as_mut_slice()).enq().expect("image.read()");
    }
}

cvar!(cap_allow_tabbing_out_in_demos, "1");
cvar!(cap_playdemostop, "1");
