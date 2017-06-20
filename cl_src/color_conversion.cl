__kernel void increase_blue(read_only image2d_t src_image,
                            write_only image2d_t dst_image) {
	int2 coord = (int2)(get_global_id(0), get_global_id(1));

	float4 pixel = read_imagef(src_image, coord);
	pixel += (float4)(0.0, 0.0, 0.5, 0.0);

	write_imagef(dst_image, coord, pixel);
}
