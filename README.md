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
1. Get stable Rust, then do `rustup target add i686-unknown-linux-gnu`
2. Install **32-bit** FFMpeg libraries. FFMpeg **3.4** is known to work; FFMpeg 4 will not work.
3. Install **32-bit** SDL2.
4. Install **32-bit** OpenCL.
5. `PKG_CONFIG_ALLOW_CROSS=1 cargo build --release`

Look at how the Travis build is set up for minimal build of the dependencies.
