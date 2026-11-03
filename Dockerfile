FROM rust:1-bookworm@sha256:93ce27a88655056a51dbdd8f5f2d7ddc071c7b0070fb288a37b5a285fc83971e AS build
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY migrations ./migrations
COPY web ./web
COPY data ./data
RUN --mount=type=cache,target=/usr/local/cargo/registry --mount=type=cache,target=/app/target \
    cargo build --release --locked && cp target/release/lesana /usr/local/bin/lesana

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates curl poppler-utils unzip tesseract-ocr tesseract-ocr-slk \
    && rm -rf /var/lib/apt/lists/* && useradd --uid 10001 --create-home lesana
COPY --from=build /usr/local/bin/lesana /usr/local/bin/lesana
USER lesana
ENV LISTEN_ADDRESS=0.0.0.0:8080
EXPOSE 8080
HEALTHCHECK --interval=20s --timeout=3s --start-period=60s CMD curl --fail http://localhost:8080/health || exit 1
ENTRYPOINT ["lesana"]
