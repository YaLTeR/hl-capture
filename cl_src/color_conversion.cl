__kernel void rgb_to_yuv444_601_limited(read_only image2d_t src_image,
                                        __global uchar* const dst_buf) {
	int x = get_global_id(0);
	int y = get_global_id(1);
	int w = get_image_width(src_image);
	int h = get_image_height(src_image);

	float4 pixel = read_imagef(src_image, (int2)(x, y));
	float Y = 16 + pixel.x * 65.481 + pixel.y * 128.553 + pixel.z * 24.966;
	float U = 128 - pixel.x * 37.797 - pixel.y * 74.203 + pixel.z * 112.0;
	float V = 128 + pixel.x * 112.0 - pixel.y * 93.786 - pixel.z * 18.214;

	dst_buf[y * w + x] = round(Y);
	dst_buf[(h * w) + y * w + x] = round(U);
	dst_buf[(h * w) * 2 + y * w + x] = round(V);
}
