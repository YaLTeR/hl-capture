#![allow(non_snake_case)]
#![allow(non_upper_case_globals)]

use failure::{Error, ResultExt};
use ffmpeg::format;
use gl;
use gl::types::*;
use glx;
use libc::*;
use ocl;
use std::cell::RefCell;
use std::cmp;
use std::ffi::{CStr, CString};
use std::mem;
use std::ptr;
use std::result;
use std::slice;

use crate::capture::{self, GameThreadEvent};
use crate::command;
use crate::cvar;
use crate::dl;
use crate::encode;
use crate::engine::MainThreadMarker;
use crate::fps_converter::*;
use crate::sdl;
use crate::utils::MaybeUnavailable;

use crate::utils::format_error;

type Result<T> = result::Result<T, Error>;

// Stuff from these variables should ONLY be accessed from the main game thread.
// TODO: move into MainThreadGlobals or something.
static mut FUNCTIONS: Option<Functions> = None;
static mut POINTERS: Option<Pointers> = None;

/// Pointers to all used hw functions.
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
                                     *const c_char) -> c_int,
    Host_FilterTime: unsafe extern "C" fn(c_float) -> c_int,
    Key_Event: unsafe extern "C" fn(key: c_int, down: c_int),
    Memory_Init: unsafe extern "C" fn(*mut c_void, c_int),
    S_PaintChannels: unsafe extern "C" fn(endtime: c_int),
    S_TransferStereo16: unsafe extern "C" fn(end: c_int),
    Sys_VID_FlipScreen: unsafe extern "C" fn(),
    VideoMode_GetCurrentVideoMode: unsafe extern "C" fn(*mut c_int, *mut c_int, *mut c_int),
    VideoMode_IsWindowed: unsafe extern "C" fn() -> c_int,
}

/// Pointers to all used hw variables.
struct Pointers {
    cls: *mut client_static_t,
    game: *mut *mut CGame,
    host_frametime: *mut c_double,
    paintbuffer: *mut portable_samplepair_t, // [1026]
    paintedtime: *mut c_int,
    realtime: *mut c_double,
    s_BackBufferFBO: *mut FBO_Container_t,
    shm: *mut *mut dma_t,
    window_rect: *mut RECT,
}

thread_local! {
    /// The audio buffer container, set and cleared in `S_PaintChannels()`.
    static AUDIO_BUFFER: RefCell<Option<capture::AudioBuffer>> = RefCell::new(None);
}

pub enum SoundCaptureMode {
    Normal,
    Remaining,
}

/// Wrapper for the OpenCL image created from an OpenGL texture.
pub struct OclGlTexture {
    image: ocl::Image<u8>,
}

pub enum FrameCapture {
    OpenGL(fn(MainThreadMarker, (u32, u32), &mut [u8])),
    OpenCL(OclGlTexture),
}

impl OclGlTexture {
    fn new(_: MainThreadMarker,
           texture: GLuint,
           queue: ocl::Queue,
           dims: ocl::SpatialDims)
           -> Self {
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

        let image =
            ocl::Image::<u8>::from_gl_texture(queue,
                                              ocl::flags::MEM_READ_ONLY,
                                              descr,
                                              ocl::core::GlTextureTarget::GlTexture2d,
                                              0,
                                              texture).expect("ocl::Image::from_gl_texture()");

        image.cmd().gl_acquire().enq().expect("gl_acquire()");

        Self { image }
    }
}

impl AsRef<ocl::Image<u8>> for OclGlTexture {
    #[inline]
    fn as_ref(&self) -> &ocl::Image<u8> {
        &self.image
    }
}

impl Drop for OclGlTexture {
    fn drop(&mut self) {
        self.image.cmd().gl_release().enq().expect("gl_release()");
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

#[repr(C)]
struct CGame {
    stuff: [u8; 0xC],
    m_hSDLGLContext: *mut c_void,
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
    let marker = MainThreadMarker::new();

    // hw.so just loaded, either for the first time or potentially at a different address.
    // Refresh all pointers.
    if let Err(e) = refresh_pointers(marker).context("error refreshing pointers") {
        panic!("{}", &format_error(&e));
    }

    // Initialize the encoding.
    encode::initialize();

    // Initialize the capturing.
    capture::initialize(marker);

    let rv = real!(RunListenServer)(instance,
                                    basedir,
                                    cmdline,
                                    postRestartCmdLineArgs,
                                    launcherFactory,
                                    filesystemFactory);

    if let Some(FPSConverters::Sampling(mut sampling_conv)) =
        marker.globals_mut().fps_converter.take()
    {
        sampling_conv.backup_and_free_ocl_data(marker);
        marker.globals_mut().fps_converter = Some(FPSConverters::Sampling(sampling_conv));
    }

    marker.globals_mut().ocl_yuv_buffers.reset();
    marker.globals_mut().pro_que.reset();

    // Since hw.so is getting unloaded, reset all pointers.
    reset_pointers(marker);

    rv
}

/// Stops the currently running game, returning to the main menu.
#[no_mangle]
pub unsafe extern "C" fn CL_Disconnect() {
    if capture::is_capturing() && (*ptr!(cls)).demoplayback != 0 {
        let marker = MainThreadMarker::new();

        if cap_playdemostop.parse(&mut *marker.engine_mut())
                           .unwrap_or(0)
           != 0
        {
            capture::stop(marker);
        }
    }

    real!(CL_Disconnect)();
}

/// Handler for the `toggleconsole` command.
#[no_mangle]
pub unsafe extern "C" fn Con_ToggleConsole_f() {
    let marker = MainThreadMarker::new();

    if !marker.globals().inside_key_event
       || cap_allow_tabbing_out_in_demos.parse(&mut *marker.engine_mut())
                                        .unwrap_or(0)
          == 0
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
    let marker = MainThreadMarker::new();
    marker.globals_mut().inside_gl_setmode = true;

    let rv = real!(GL_SetMode)(mainwindow, pmaindc, pbaseRC, fD3D, pszDriver, pszCmdLine);

    marker.globals_mut().inside_gl_setmode = false;

    rv
}

/// Calculates the frame time and limits the FPS.
#[no_mangle]
pub unsafe extern "C" fn Host_FilterTime(time: c_float) -> c_int {
    let marker = MainThreadMarker::new();

    let old_realtime = *ptr!(realtime);

    let rv = real!(Host_FilterTime)(time);

    // TODO: this will NOT set the frametime on the first frame of capture / demo playback and WILL
    // set the frametime on the first frame of not capturing. This needs to be fixed somehow.
    if capture::is_capturing() && (*ptr!(cls)).demoplayback != 0 {
        let params = capture::get_capture_parameters(marker);
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
    let marker = MainThreadMarker::new();
    marker.globals_mut().inside_key_event = true;
    real!(Key_Event)(key, down);
    marker.globals_mut().inside_key_event = false;
}

/// Initializes the hunk memory.
///
/// After the hunk memory has been initialized we can register console commands and variables.
/// It's also a good place to get OpenGL function pointers.
#[no_mangle]
pub unsafe extern "C" fn Memory_Init(buf: *mut c_void, size: c_int) {
    real!(Memory_Init)(buf, size);

    let marker = MainThreadMarker::new();
    register_cvars_and_commands(marker);

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
    let marker = MainThreadMarker::new();

    if !capture::is_capturing() {
        marker.globals_mut().capture_sound = false;
        real!(S_PaintChannels)(endtime);
        return;
    }

    if marker.globals().capture_sound {
        let paintedtime = *ptr!(paintedtime);
        let frametime = match marker.globals().sound_capture_mode {
            SoundCaptureMode::Normal => *ptr!(host_frametime),
            SoundCaptureMode::Remaining => capture::get_capture_parameters(marker).sound_extra,
        };
        let speed = (**ptr!(shm)).speed;
        let samples = frametime * f64::from(speed) + marker.globals().sound_remainder;
        let samples_rounded = match marker.globals().sound_capture_mode {
            SoundCaptureMode::Normal => samples.floor(),
            SoundCaptureMode::Remaining => samples.ceil(),
        };

        marker.globals_mut().sound_remainder = samples - samples_rounded;

        AUDIO_BUFFER.with(|b| {
                        let mut buf = capture::get_audio_buffer(marker);
                        buf.data_mut().clear();
                        *b.borrow_mut() = Some(buf);
                    });

        real!(S_PaintChannels)(paintedtime + samples_rounded as i32);

        AUDIO_BUFFER.with(|b| capture::capture_audio(marker, b.borrow_mut().take().unwrap()));

        marker.globals_mut().capture_sound = false;
    }
}

/// Transfers the contents of the paintbuffer into the output buffer.
#[no_mangle]
pub unsafe extern "C" fn S_TransferStereo16(end: c_int) {
    let marker = MainThreadMarker::new();
    if marker.globals().capture_sound {
        AUDIO_BUFFER.with(|b| {
                        let mut buf = b.borrow_mut();
                        let buf = buf.as_mut().unwrap().data_mut();

                        let paintedtime = *ptr!(paintedtime);
                        let paintbuffer = slice::from_raw_parts_mut(ptr!(paintbuffer), 1026);

                        let marker = MainThreadMarker::new();
                        let volume =
                            (capture::get_capture_parameters(marker).volume * 256f32) as i32;

                        for sample in paintbuffer.iter().take((end - paintedtime) as usize * 2) {
                            // Clamping as done in Snd_WriteLinearBlastStereo16().
                            let l16 = cmp::min(32767, cmp::max(-32768, (sample.left * volume) >> 8))
                                      as i16;
                            let r16 = cmp::min(32767,
                                               cmp::max(-32768, (sample.right * volume) >> 8))
                                      as i16;

                            buf.push((l16, r16));
                        }
                    });
    }

    real!(S_TransferStereo16)(end);
}

/// Flips the screen.
#[export_name = "_Z18Sys_VID_FlipScreenv"]
pub unsafe extern "C" fn Sys_VID_FlipScreen() {
    let marker = MainThreadMarker::new();

    // Print all messages that happened.
    while let Some(e) = capture::get_event(marker) {
        match e {
            GameThreadEvent::Message(msg) => con_print(&msg),
            GameThreadEvent::EncoderPixelFormat(fmt) => {
                marker.globals_mut().encoder_pixel_format = Some(fmt)
            }
        }
    }

    // If the encoding just started, wait for the pixel format.
    while capture::is_capturing() && marker.globals().encoder_pixel_format.is_none() {
        match capture::get_event_block(marker) {
            GameThreadEvent::Message(msg) => con_print(&msg),
            GameThreadEvent::EncoderPixelFormat(fmt) => {
                marker.globals_mut().encoder_pixel_format = Some(fmt)
            }
        }
    }

    if capture::is_capturing() {
        // Always capture sound.
        marker.globals_mut().capture_sound = true;

        let converter = marker.globals_mut().fps_converter.take().unwrap();
        match converter {
            FPSConverters::Simple(mut simple_conv) => {
                simple_conv.time_passed(marker, *ptr!(host_frametime), capture_frame);
                marker.globals_mut().fps_converter = Some(FPSConverters::Simple(simple_conv));
            }

            FPSConverters::Sampling(mut sampling_conv) => {
                sampling_conv.time_passed(marker, *ptr!(host_frametime), capture_frame);
                marker.globals_mut().fps_converter = Some(FPSConverters::Sampling(sampling_conv));
            }
        }
    }

    real!(Sys_VID_FlipScreen)();

    // TODO: check if we're called from SCR_UpdateScreen().
}

/// Returns whether the game is running in windowed mode.
#[no_mangle]
pub unsafe fn VideoMode_IsWindowed() -> c_int {
    // Forcing FBO usage is temporarily disabled as it causes issues when resizing the window.
    // let engine = Engine::new();
    //
    // // Force FBO usage.
    // if marker.globals().inside_gl_setmode {
    //     return 0;
    // }

    real!(VideoMode_IsWindowed)()
}

/// Obtains and stores all necessary function and variable addresses.
fn refresh_pointers(_: MainThreadMarker) -> Result<()> {
    let hw = dl::open("hw.so", RTLD_NOW | RTLD_NOLOAD).context("couldn't load hw.so")?;

    unsafe {
        FUNCTIONS = Some(Functions { RunListenServer: find!(
            hw,
            "_Z15RunListenServerPvPcS0_S0_PFP14IBaseInterfacePKcPiES7_"
        ),
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
                                     VideoMode_GetCurrentVideoMode: find!(
            hw,
            "VideoMode_GetCurrentVideoMode"
        ),
                                     VideoMode_IsWindowed: find!(hw, "VideoMode_IsWindowed"), });

        POINTERS = Some(Pointers { cls: find!(hw, "cls"),
                                   game: find!(hw, "game"),
                                   host_frametime: find!(hw, "host_frametime"),
                                   paintbuffer: find!(hw, "paintbuffer"),
                                   paintedtime: find!(hw, "paintedtime"),
                                   realtime: find!(hw, "realtime"),
                                   s_BackBufferFBO: find!(hw, "s_BackBufferFBO"),
                                   shm: find!(hw, "shm"),
                                   window_rect: find!(hw, "window_rect") });
    }

    Ok(())
}

/// Resets all pointers to their default values.
#[inline]
fn reset_pointers(_: MainThreadMarker) {
    unsafe {
        FUNCTIONS = None;
        POINTERS = None;
    }
}

/// Registers console commands and variables.
fn register_cvars_and_commands(marker: MainThreadMarker) {
    for cmd in &command::COMMANDS {
        unsafe {
            register_command(cmd.name(), cmd.callback());
        }
    }

    let engine = &mut *marker.engine_mut();
    for cvar in &cvar::CVARS {
        if let Err(e) = cvar.register(engine)
                            .context("error registering a console variable")
        {
            panic!("{}", format_error(&e));
        }
    }
}

/// Registers a console command.
///
/// # Safety
/// Unsafe because this function should only be called from the main game thread.
#[inline]
unsafe fn register_command(name: &'static [u8], callback: unsafe extern "C" fn()) {
    real!(Cmd_AddCommand)(name as *const _ as *const _, callback as *mut c_void);
}

/// Registers a console variable.
///
/// # Safety
/// Unsafe because this function should only be called from the main game thread.
#[inline]
pub unsafe fn register_variable(cvar: &mut cvar::cvar_t) {
    real!(Cvar_RegisterVariable)(cvar);
}

/// Prints the given string to the game console.
///
/// `string` must not contain null bytes.
///
/// # Safety
/// Unsafe because this function should only be called from the main game thread.
#[inline]
pub unsafe fn con_print(string: &str) {
    let cstring =
        CString::new(string.replace('%', "%%")).expect("string cannot contain null bytes");
    real!(Con_Printf)(cstring.as_ptr())
}

/// Gets the console command argument count.
///
/// # Safety
/// Unsafe because this function should only be called from the main game thread.
#[inline]
pub unsafe fn cmd_argc() -> u32 {
    let argc = real!(Cmd_Argc)();
    cmp::max(0, argc) as u32
}

/// Gets a console command argument.
///
/// # Safety
/// Unsafe because this function should only be called from the main game thread.
#[inline]
pub unsafe fn cmd_argv(index: u32) -> String {
    let index = cmp::min(index, i32::max_value() as u32) as i32;
    let arg = real!(Cmd_Argv)(index);
    CStr::from_ptr(arg).to_string_lossy().into_owned()
}

/// Returns the current game resolution.
pub fn get_resolution(_: MainThreadMarker) -> (u32, u32) {
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
#[inline]
pub fn reset_sound_capture_remainder(marker: MainThreadMarker) {
    marker.globals_mut().sound_remainder = 0f64;
}

/// Captures the remaining and extra sound.
#[inline]
pub fn capture_remaining_sound(marker: MainThreadMarker) {
    marker.globals_mut().sound_capture_mode = SoundCaptureMode::Remaining;
    marker.globals_mut().capture_sound = true;
    unsafe {
        S_PaintChannels(0);
    }
    marker.globals_mut().sound_capture_mode = SoundCaptureMode::Normal;
}

/// Returns the ocl `ProCue` after potentially initializing it.
pub fn get_pro_que(marker: MainThreadMarker,
                   pro_que: &mut MaybeUnavailable<ocl::ProQue>)
                   -> Option<&mut ocl::ProQue> {
    if pro_que.is_not_checked() {
        let context = ocl::Context::builder().gl_context(get_opengl_context(marker))
                                             .glx_display(unsafe { glx::GetCurrentDisplay() } as _)
                                             .build()
                                             .context("error building ocl::Context");

        let new_pro_que = context.and_then(|ctx| {
                                     ocl::ProQue::builder().context(ctx)
                                                           .prog_bldr({
                                                               let mut builder =
                                                                   ocl::Program::builder();
                                                               builder
                            .src(include_str!("../../cl_src/color_conversion.cl"))
                            .src(include_str!("../../cl_src/sampling.cl"));
                                                               builder
                                                           })
                                                           .build()
                                                           .context("error building ocl::ProQue")
                                 })
                                 .map_err(|e| {
                                     marker.engine()
                                           .con_print(&format!("Could not initialize OpenCL, \
                                                                proceeding without it. \
                                                                Error details:\n{}",
                                                               format_error(&e)).replace('\0', "\\x00"));
                                 })
                                 .ok();

        *pro_que = MaybeUnavailable::from_check_result(new_pro_que);
    }

    pro_que.as_mut().available()
}

/// Builds an ocl `Buffer` with the specified length.
fn build_ocl_buffer(pro_que: &ocl::ProQue, length: usize) -> Result<ocl::Buffer<u8>> {
    Ok(ocl::Buffer::<u8>::builder().queue(pro_que.queue().clone())
                                   .flags(ocl::flags::MemFlags::new().write_only().host_read_only())
                                   .len(length)
                                   .build()
                                   .context("could not build the OpenCL buffer")?)
}

/// Builds an ocl `Image` with the specified dimensions.
pub fn build_ocl_image<T: ocl::OclPrm>(pro_que: &ocl::ProQue,
                                       mem_flags: ocl::MemFlags,
                                       data_type: ocl::enums::ImageChannelDataType,
                                       dims: ocl::SpatialDims)
                                       -> Result<ocl::Image<T>> {
    Ok(ocl::Image::<T>::builder().channel_order(ocl::enums::ImageChannelOrder::Rgba)
                                 .channel_data_type(data_type)
                                 .image_type(ocl::enums::MemObjectType::Image2d)
                                 .dims(dims)
                                 .flags(mem_flags)
                                 .queue(pro_que.queue().clone())
                                 .build()
                                 .context("could not build the OpenCL image")?)
}

/// Builds ocl YUV buffers with the specified length.
fn build_yuv_buffers(pro_que: &ocl::ProQue,
                     (Y_len, U_len, V_len): (usize, usize, usize))
                     -> Option<(ocl::Buffer<u8>, ocl::Buffer<u8>, ocl::Buffer<u8>)> {
    let Y_buf = build_ocl_buffer(pro_que, Y_len);
    let U_buf = build_ocl_buffer(pro_que, U_len);
    let V_buf = build_ocl_buffer(pro_que, V_len);

    if let (Ok(Y_buf), Ok(U_buf), Ok(V_buf)) = (Y_buf, U_buf, V_buf) {
        Some((Y_buf, U_buf, V_buf))
    } else {
        None
    }
}

/// Returns the ocl buffers for Y, U and V after potentially (re-)creating them.
fn get_yuv_buffers<'yuv_buffers>(
    pro_que: &ocl::ProQue,
    ocl_yuv_buffers: &'yuv_buffers mut MaybeUnavailable<(ocl::Buffer<u8>,
                                        ocl::Buffer<u8>,
                                        ocl::Buffer<u8>)>,
    (Y_len, U_len, V_len): (usize, usize, usize))
    -> Option<&'yuv_buffers mut (ocl::Buffer<u8>, ocl::Buffer<u8>, ocl::Buffer<u8>)> {
    // If the buffers don't exist just create them.
    if !ocl_yuv_buffers.is_available() {
        let buffers = build_yuv_buffers(pro_que, (Y_len, U_len, V_len));
        *ocl_yuv_buffers = MaybeUnavailable::from_check_result(buffers);
        return ocl_yuv_buffers.as_mut().available();
    }

    // Check if the requested buffer size is different.
    // In most cases if one of the buffer sizes changes, the other do as well.
    let (Y_buf, U_buf, V_buf) = ocl_yuv_buffers.as_mut().unwrap();

    if Y_buf.len() != Y_len || U_buf.len() != U_len || V_buf.len() != V_len {
        // First drop the existing buffers.
        drop(ocl_yuv_buffers.take());

        // Now allocate new ones.
        let buffers = build_yuv_buffers(pro_que, (Y_len, U_len, V_len));
        *ocl_yuv_buffers = MaybeUnavailable::from_check_result(buffers);
    }

    ocl_yuv_buffers.as_mut().available()
}

/// Captures and returns the current frame.
fn capture_frame(marker: MainThreadMarker) -> FrameCapture {
    let texture = unsafe { *ptr!(s_BackBufferFBO) }.Tex;

    let pro_que = &mut marker.globals_mut().pro_que;
    let pro_que = if texture != 0 {
        get_pro_que(marker, pro_que)
    } else {
        None
    };

    if let Some(pro_que) = pro_que {
        let (w, h) = get_resolution(marker);

        FrameCapture::OpenCL(OclGlTexture::new(marker,
                                               texture,
                                               pro_que.queue().clone(),
                                               (w, h).into()))
    } else {
        FrameCapture::OpenGL(read_pixels)
    }
}

/// Gets the OpenCL RGB->YUV color conversion function name.
fn ocl_color_conversion_func_name(target: format::Pixel) -> Option<&'static str> {
    match target {
        format::Pixel::YUV420P => Some("rgb_to_yuv420_601_limited"),
        format::Pixel::YUV444P => Some("rgb_to_yuv444_601_limited"),
        _ => None,
    }
}

/// Reads the given `ocl::Image` into the buffer.
pub fn read_ocl_image_into_buf<T: ocl::OclPrm>(marker: MainThreadMarker,
                                               image: &ocl::Image<T>,
                                               buf: &mut capture::VideoBuffer) {
    let globals = &mut *marker.globals_mut();
    let pro_que = get_pro_que(marker, &mut globals.pro_que).unwrap();
    let encoder_pixel_format = globals.encoder_pixel_format.unwrap();

    if let Some(func_name) = ocl_color_conversion_func_name(encoder_pixel_format) {
        buf.set_format(encoder_pixel_format);
        let frame = buf.get_frame();

        if let Some(&mut (ref Y_buf, ref U_buf, ref V_buf)) =
            get_yuv_buffers(pro_que,
                            &mut globals.ocl_yuv_buffers,
                            (frame.data(0).len(), frame.data(1).len(), frame.data(2).len()))
        {
            let kernel = pro_que.kernel_builder(func_name)
                                .global_work_size(image.dims())
                                .arg(image)
                                .arg(frame.stride(0))
                                .arg(frame.stride(1))
                                .arg(frame.stride(2))
                                .arg(Y_buf)
                                .arg(U_buf)
                                .arg(V_buf)
                                .build()
                                .unwrap();

            unsafe {
                kernel.enq().expect("kernel.enq()");
            }

            Y_buf.read(frame.data_mut(0)).enq().expect("Y_buf.read()");
            U_buf.read(frame.data_mut(1)).enq().expect("U_buf.read()");
            V_buf.read(frame.data_mut(2)).enq().expect("V_buf.read()");

            return;
        }
    }

    buf.set_format(format::Pixel::RGBA);

    let ocl_buffer =
        build_ocl_buffer(pro_que, buf.as_mut_slice().len()).expect("OpenCL buffer build");

    let kernel = pro_que.kernel_builder("rgba_to_uint8_rgba_buffer")
                        .global_work_size(image.dims())
                        .arg(image)
                        .arg(&ocl_buffer)
                        .build()
                        .unwrap();

    unsafe {
        kernel.enq().expect("kernel.enq()");
    }

    ocl_buffer.read(buf.as_mut_slice())
              .enq()
              .expect("buffer.read()");
}

/// Reads pixels into the buffer.
fn read_pixels(_: MainThreadMarker, (w, h): (u32, u32), buf: &mut [u8]) {
    unsafe {
        // Our buffer expects 1-byte alignment.
        gl::PixelStorei(gl::PACK_ALIGNMENT, 1);

        // Get the pixels!
        gl::ReadPixels(0,
                       0,
                       w as GLsizei,
                       h as GLsizei,
                       gl::RGB,
                       gl::UNSIGNED_BYTE,
                       buf.as_mut_ptr() as _);
    }
}

/// Retrieves the current OpenGL context.
fn get_opengl_context(_: MainThreadMarker) -> *mut c_void {
    unsafe { (**ptr!(game)).m_hSDLGLContext }
}

cvar!(cap_allow_tabbing_out_in_demos, "1");
cvar!(cap_playdemostop, "1");
