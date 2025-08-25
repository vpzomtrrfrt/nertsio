FROM alpine:3.22 AS builder

RUN apk add --no-cache cargo pkgconf openssl-dev

WORKDIR /usr/src/nertsio

RUN sh -c "echo -e '[workspace]\nmembers = [\"types\", \"common\", \"overseer\"]' > Cargo.toml"
COPY Cargo.lock ./
COPY overseer ./overseer
COPY common ./common
COPY types ./types

RUN cd overseer
RUN cargo build --release

FROM alpine:3.22

RUN apk add --no-cache openssl libgcc

RUN adduser -S overseer

COPY --from=builder /usr/src/nertsio/target/release/nertsio_overseer /usr/bin/

USER overseer
CMD ["nertsio_overseer"]
