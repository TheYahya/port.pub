FROM rust:1.85 as builder

WORKDIR /app
COPY . .
RUN cargo build -p server --release

FROM rust:1.85
WORKDIR /app
COPY --from=builder /app/target/release/server .
EXPOSE 8080
CMD ["./server"]

