import time

from ffmpeg_input import FFMpegSource, FFmpegLogLevel

if __name__ == '__main__':
    s = FFMpegSource("/dev/video0",
                     params={"video_size": "1920x1080", "c:v": "v4l2m2m", "input_format": "yuyv422"},
                     queue_len=100,
                     autoconvert_raw_formats_to_rgb24=True,
                     ffmpeg_log_level=FFmpegLogLevel.Info)
    s.log_level = FFmpegLogLevel.Panic
    while True:
        try:
            p = s.video_frame()
            res = p.payload_as_bytes()
            # 1944 2592
            # print(p.frame_height, p.frame_width)
            # res = np.frombuffer(res, dtype=np.uint8)
            # res = np.reshape(res, (p.frame_height, p.frame_width, 3))
            end = time.time()
            print(p.codec, p.pixel_format, p.queue_len, "all_time={}".format(int(end * 1000 - p.frame_received_ts)),
                  "python_time={}".format(int(end * 1000 - p.frame_processed_ts)))
            # cv2.imshow('Image', res)
            # if cv2.waitKey(1) & 0xFF == ord('q'):
            #    break
        except BrokenPipeError:
            print("EOS")
            break
