FROM gcr.io/distroless/cc-debian12
COPY ./target/release/samousse-rs /
CMD ["./samousse-rs"]