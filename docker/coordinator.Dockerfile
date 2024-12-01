FROM alpine:3.19 AS builder

RUN apk add --no-cache cargo pkgconf openssl-dev

WORKDIR /usr/src/nertsio

RUN sh -c "echo -e '[workspace]\nmembers = [\"types\", \"coordinator\"]' > Cargo.toml"
COPY Cargo.lock ./
COPY coordinator ./coordinator
COPY types ./types

RUN cd coordinator
RUN cargo build --release

FROM alpine:3.19

RUN apk add --no-cache openssl libgcc

RUN adduser -S coordinator

COPY --from=builder /usr/src/nertsio/target/release/nertsio_coordinator /usr/bin/

USER coordinator
CMD ["nertsio_coordinator"]
