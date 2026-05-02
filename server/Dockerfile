FROM --platform=$BUILDPLATFORM golang:1.25-bookworm AS builder

WORKDIR /src

ARG BUILDPLATFORM
ARG TARGETOS
ARG TARGETARCH
ARG TARGETVARIANT

COPY go.mod go.sum ./
RUN go mod download

COPY . .
RUN set -eux; \
    GOOS_VALUE="${TARGETOS:-$(go env GOOS)}"; \
    GOARCH_VALUE="${TARGETARCH:-$(go env GOARCH)}"; \
    export GOOS="$GOOS_VALUE"; \
    export GOARCH="$GOARCH_VALUE"; \
    if [ "$GOARCH_VALUE" = "arm" ] && [ -n "$TARGETVARIANT" ]; then export GOARM="${TARGETVARIANT#v}"; fi; \
    CGO_ENABLED=0 go build -trimpath -ldflags="-s -w" -o /out/eggs-server .

FROM debian:bookworm-slim

RUN useradd --system --create-home --home-dir /app appuser

WORKDIR /app

COPY --from=builder /out/eggs-server /usr/local/bin/eggs-server

RUN mkdir -p /data && chown -R appuser:appuser /data /app

USER appuser

EXPOSE 8787

ENTRYPOINT ["eggs-server"]
CMD ["-addr", ":8787", "-data", "/data", "-base-url", "http://localhost:8787"]
