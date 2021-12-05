FROM alpine:3.15 AS builder

RUN apk add --no-cache cargo pkgconf openssl-dev

WORKDIR /usr/src/nertsio

RUN sh -c "echo -e '[workspace]\nmembers = [\"types\", \"gameserver\"]' > Cargo.toml"
COPY Cargo.lock ./
COPY gameserver ./gameserver
COPY types ./types

RUN cd gameserver
RUN cargo build --release

FROM alpine:3.15

RUN apk add --no-cache openssl libgcc

RUN adduser -S gameserver

COPY --from=builder /usr/src/nertsio/target/release/nertsio_gameserver /usr/bin/

USER gameserver
CMD ["nertsio_gameserver"]
