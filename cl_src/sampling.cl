__kernel void weighted_image_add(read_only image2d_t src_image,
                                 read_only image2d_t buf_image,
                                 write_only image2d_t dst_image,
                                 __private float const weight) {
	int2 coords = (int2)(get_global_id(0), get_global_id(1));

	float4 src_pixel = read_imagef(src_image, coords);
	float4 buf_pixel = read_imagef(buf_image, coords);
	float4 pixel = buf_pixel + weight * src_pixel;

	write_imagef(dst_image, coords, pixel);
}
