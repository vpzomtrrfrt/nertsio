FROM alpine:3.22 AS builder

RUN apk add --no-cache cargo pkgconf openssl-dev

WORKDIR /usr/src/nertsio

RUN sh -c "echo -e '[workspace]\nmembers = [\"types\", \"common\", \"coordinator\"]' > Cargo.toml"
COPY Cargo.lock ./
COPY coordinator ./coordinator
COPY common ./common
COPY types ./types

RUN cd coordinator
RUN cargo build --release

FROM alpine:3.22

RUN apk add --no-cache openssl libgcc

RUN adduser -S coordinator

COPY --from=builder /usr/src/nertsio/target/release/nertsio_coordinator /usr/bin/

USER coordinator
CMD ["nertsio_coordinator"]
