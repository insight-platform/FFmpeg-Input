import os

os.environ["RUST_LOG"] = "info"

from ffmpeg_input import FFMpegSource, FFmpegLogLevel, BsfFilter


def bytes_to_bits_binary(byte_data):
    bits_data = bin(int.from_bytes(byte_data, byteorder='big'))
    return bits_data


if __name__ == '__main__':
    # set env LOGLEVEL=info
    # file = "/dev/video0"
    file = "/home/ivan/Downloads/1_underground_supercut.mp4"
    # file = "/home/ivan/Downloads/1_underground_supercut_reencode_bug_x265.mp4"
    # file = "/home/ivan/Downloads/1_underground_supercut_reencode_bug_aud.mp4"
    s = FFMpegSource(file, params=[],
                     queue_len=10, decode=False,
                     block_if_queue_full=True,
                     ffmpeg_log_level=FFmpegLogLevel.Info,
                     bsf_filters=[BsfFilter("h264", "h264_mp4toannexb"),
                                  BsfFilter("hevc", "hevc_mp4toannexb"),
                                  BsfFilter("h265", "hevc_mp4toannexb")])
    s.log_level = FFmpegLogLevel.Info
    # counter = 0
    f = open("output.h264", "wb")
    while True:
        try:
            p = s.video_frame()
            print("Codec: {}, Geometry: {}x{}".format(p.codec, p.frame_width, p.frame_height))
            print("System ts, when the frame was received from the source:", p.frame_received_ts)
            print("Current queue length:", p.queue_len)
            print("Time base, pts, dts:", p.time_base, p.pts, p.dts)
            print("Skipped frames because of queue overflow:", p.queue_full_skipped_count)
            # print("Is bytestream", p.is_byte_stream)
            payload = p.payload_as_bytes()
            f.write(payload)
            print("Payload length:", len(payload))
            # print 1st 3 bytes of the payload
            # bin_res = " ".join(format(x, '#010b')[2:] for x in payload[:16])
            # first_hex_res = " ".join(format(x, '02x') for x in payload[:16])
            # last_hex_res = " ".join(format(x, '02x') for x in payload[-4:])
            # code = decode_exp_golomb(payload[:16])
            # # int_val = int(code, 2)
            # print("Payload bin start:", code)
            # print("Payload hex start:", first_hex_res)
            # if len(payload) - 4 > int_val:
            #     first_hex_res = " ".join(format(x, '02x') for x in payload[4 + int_val:20 + int_val])
            #     print("Payload hex start:", first_hex_res)
            # # print("Payload start:", bytes_to_bits_binary(payload[:16]))
            # if counter == 20:
            #     s.stop()
            #     break
            # counter += 1
        except BrokenPipeError:
            print("EOS")
            break
