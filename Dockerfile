FROM python:3.8-buster as base

WORKDIR /opt
COPY docker/install-basic-deps.sh .
RUN bash /opt/install-basic-deps.sh

FROM base as chef
ENV PATH="/root/.cargo/bin:$PATH"
RUN rustc -V

FROM chef AS planner
WORKDIR /opt
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder
WORKDIR /opt
COPY --from=planner /opt/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json
COPY . .

ENV RUSTFLAGS=" -C target-cpu=native -C opt-level=3"
ENV LD_LIBRARY_PATH="/usr/lib/x86_64-linux-gnu/pulseaudio"

RUN maturin build --release --out dist
RUN python3 -m pip install --upgrade pip
RUN python3 -m pip install dist/*.whl
