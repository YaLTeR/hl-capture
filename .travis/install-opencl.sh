#!/bin/sh
set -ex

if [ ! -d "$HOME/OpenCL/lib" ]; then
	mkdir "OpenCL"
	cd "OpenCL"
	wget "https://archive.archlinux.org/packages/l/lib32-ocl-icd/lib32-ocl-icd-2.2.11-1-x86_64.pkg.tar.xz"
	tar xJf "lib32-ocl-icd-2.2.11-1-x86_64.pkg.tar.xz"
	mkdir -p "$HOME/OpenCL/lib"
	mv usr/lib32/* "$HOME/OpenCL/lib/"
else
	echo "Using cached directory."
fi
