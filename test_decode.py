from ffmpeg_input import FFMpegSource, VideoFrameEnvelope
import numpy as np
import cv2
import time

x = 1296
y = 972
h = 200
w = 800

if __name__ == '__main__':
    s = FFMpegSource("dump.mp4", {"c:v": "v4l2m2m"}, len=100, decode=True)
    while True:
        try:
            start = time.time()
            p = s.video_frame()
            res = p.payload_as_bytes()
            res = np.frombuffer(res, dtype=np.uint8)
            res = np.reshape(res, (p.frame_height, p.frame_width, 3))
            res = cv2.rotate(res[y - h:y + h, x:x + w], cv2.ROTATE_90_COUNTERCLOCKWISE)
            end = time.time()
            cv2.imshow('Image', res)
            print(p.queue_len, end - start)
            if cv2.waitKey(1) & 0xFF == ord('q'):
                break
        except BrokenPipeError:
            print("EOS")
            break
