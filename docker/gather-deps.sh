set -e
mkdir -p deps
ldd ./target/release/libffmpeg_input.so | awk '{print $3}' | xargs -I{} cp -L --parents {} deps/
