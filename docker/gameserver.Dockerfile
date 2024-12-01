FROM alpine:3.19 AS builder

RUN apk add --no-cache cargo pkgconf openssl-dev

WORKDIR /usr/src/nertsio

RUN sh -c "echo -e '[workspace]\nmembers = [\"types\", \"gameserver\", \"ui_metrics\", \"common\"]' > Cargo.toml"
COPY Cargo.lock ./
COPY common ./common
COPY gameserver ./gameserver
COPY types ./types
COPY ui_metrics ./ui_metrics

RUN cd gameserver
RUN cargo build --release

FROM alpine:3.19

RUN apk add --no-cache openssl libgcc

RUN adduser -S gameserver

COPY --from=builder /usr/src/nertsio/target/release/nertsio_gameserver /usr/bin/

USER gameserver
CMD ["nertsio_gameserver"]
