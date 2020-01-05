# hl-capture

[![Build Status](https://travis-ci.org/YaLTeR/hl-capture.svg?branch=master)](https://travis-ci.org/YaLTeR/hl-capture)

[Internal rustdoc](https://yalter.github.io/hl-capture)

hl-capture is a tool for recording Half-Life videos on Linux, written in Rust. It's similar to [Half-Life Advanced Effects](http://www.advancedfx.org/), but focuses on video capturing rather than advanced movie-making functionality.

hl-capture is designed to be **fast** and **convenient**. Video and sound are encoded with [FFMpeg](http://ffmpeg.org/) right away into any desirable format like `mp4`, `mkv` or `webm`. This, together with utilizing multiple threads and GPU-accelerated processing, makes hl-capture way faster than HLAE or Source's startmovie.

## Features
- Fast video and sound capturing and encoding into almost any of the formats supported by FFMpeg.
- GPU-accelerated resampling.
- TAS compatibility out of the box, including engine restarts.

## Requirements
- 32-bit FFMpeg libraries. FFMpeg **3.4** is known to work; FFMpeg 4 will not work. You can download pre-built working libraries with the main codecs [here](https://mega.nz/#!1JRAyD7Z!w0cWQIznCRGQz8ovXO4hKKDBFvgU4BbYrDVosOEoZHU).
- 32-bit OpenCL (look for something like ocl-icd).

## Usage
Check the [wiki](https://github.com/YaLTeR/hl-capture/wiki/Installation-and-usage) for installation and usage instructions.

## Goals
- High capturing speed.
- Compatibility with TASes, including those utilizing engine restarts and RNG manipulation.

## Building
1. Get Rust 1.38.0 with the `i686-unknown-linux-gnu` target.
    - This can be done using rustup with:
      ```
      rustup toolchain add 1.38.0
      rustup run 1.38.0 rustup target add i686-unknown-linux-gnu
      ```
    - Rust 1.38.0 is used due to building issues with the `ffmpeg` crate which, unfortunately, went unmaintained several years ago.
2. Install **32-bit** FFMpeg libraries. FFMpeg **3.4** is known to work; FFMpeg 4 will not work.
    - Use the following commands for a minimal build of FFMpeg known to work for building hl-capture:
      ```
      git clone --depth=1 --branch=release/3.4 https://github.com/FFMpeg/FFMpeg.git ffmpeg
      cd ffmpeg
      ./configure --disable-programs --disable-doc --enable-cross-compile --arch=x86_32 --target_os=linux --prefix="$PWD/install" --cc="gcc -m32" --disable-static --enable-shared
      make && make install
      ```
3. Install **32-bit** SDL2.
4. Install **32-bit** OpenCL.
5. `PKG_CONFIG_ALLOW_CROSS=1 cargo +1.38.0 build --release`
    - If you built FFMpeg manually, use the following command:
      ```
      PKG_CONFIG_ALLOW_CROSS=1 PKG_CONFIG_PATH=/path/to/ffmpeg/install/lib/pkgconfig LD_LIBRARY_PATH=/path/to/ffmpeg/install/lib cargo +1.38.0 build --release
      ```

Look at how the Travis build is set up for minimal build of the dependencies.
