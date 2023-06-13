from ffmpeg_input import FFMpegSource, FFmpegLogLevel
import numpy as np
import cv2
import time

x = 500
y = 500
h = 200
w = 600

if __name__ == '__main__':
    s = FFMpegSource("/dev/video0",
                     params={"video_size": "1280x720", "c:v": "v4l2m2m", "input_format": "mjpeg"},
                     queue_len=100,
                     decode=True,
                     ffmpeg_log_level=FFmpegLogLevel.Info)
    s.log_level = FFmpegLogLevel.Panic
    while True:
        try:
            p = s.video_frame()
            res = p.payload_as_bytes()
            # 1944 2592
            print(p.frame_height, p.frame_width)
            res = np.frombuffer(res, dtype=np.uint8)
            res = np.reshape(res, (p.frame_height, p.frame_width, 3))
            end = time.time()
            print(p.queue_len, "all_time={}".format(int(end * 1000 - p.frame_received_ts)),
                  "python_time={}".format(int(end * 1000 - p.frame_processed_ts)))
            cv2.imshow('Image', res)
            if cv2.waitKey(1) & 0xFF == ord('q'):
                break
        except BrokenPipeError:
            print("EOS")
            break
