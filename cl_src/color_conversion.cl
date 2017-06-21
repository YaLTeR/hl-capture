__kernel void rgb_to_yuv444_601_limited(read_only image2d_t src_image,
                                        __private uint const Y_stride,
                                        __private uint const U_stride,
                                        __private uint const V_stride,
                                        __global uchar* const Y_buf,
                                        __global uchar* const U_buf,
                                        __global uchar* const V_buf) {
	int2 coords = (int2)(get_global_id(0), get_global_id(1));

	float4 pixel = read_imagef(src_image, coords);
	float Y = 16 + pixel.x * 65.481 + pixel.y * 128.553 + pixel.z * 24.966;
	float U = 128 - pixel.x * 37.797 - pixel.y * 74.203 + pixel.z * 112.0;
	float V = 128 + pixel.x * 112.0 - pixel.y * 93.786 - pixel.z * 18.214;

	// FFMpeg frames are flipped.
	coords.y = get_image_height(src_image) - coords.y - 1;

	Y_buf[coords.y * Y_stride + coords.x] = round(Y);
	U_buf[coords.y * U_stride + coords.x] = round(U);
	V_buf[coords.y * V_stride + coords.x] = round(V);
}

__kernel void rgb_to_yuv420_601_limited(read_only image2d_t src_image,
                                        __private uint const Y_stride,
                                        __private uint const U_stride,
                                        __private uint const V_stride,
                                        __global uchar* const Y_buf,
                                        __global uchar* const U_buf,
                                        __global uchar* const V_buf) {
	int2 coords = (int2)(get_global_id(0), get_global_id(1));
	int h = get_image_height(src_image);

	float4 pixel = read_imagef(src_image, coords);

	float Y = 16 + pixel.x * 65.481 + pixel.y * 128.553 + pixel.z * 24.966;
	Y_buf[(h - coords.y - 1) * Y_stride + coords.x] = round(Y);

	if ((coords.x & 1) == 0 && (coords.y & 1) == 0) {
		// Average the 4 pixel values for better results.
		int w = get_image_width(src_image);
		float4 top_right, bottom_left, bottom_right;

		if (coords.x + 1 < w) {
			top_right = read_imagef(src_image, coords + (int2)(1, 0));
		} else {
			top_right = pixel;
		}

		if (coords.y + 1 < h) {
			bottom_left = read_imagef(src_image, coords + (int2)(0, 1));
		} else {
			bottom_left = pixel;
		}

		if (coords.x + 1 < w && coords.y + 1 < h) {
			bottom_right = read_imagef(src_image, coords + (int2)(1, 1));
		} else {
			bottom_right = pixel;
		}

		pixel = (pixel + top_right + bottom_left + bottom_right) / (float4)(4.0, 4.0, 4.0, 4.0);

		coords.x >>= 1;
		coords.y >>= 1;

		coords.y = (h >> 1) - coords.y - 1;

		float U = 128 - pixel.x * 37.797 - pixel.y * 74.203 + pixel.z * 112.0;
		float V = 128 + pixel.x * 112.0 - pixel.y * 93.786 - pixel.z * 18.214;

		U_buf[coords.y * U_stride + coords.x] = round(U);
		V_buf[coords.y * V_stride + coords.x] = round(V);
	}
}
