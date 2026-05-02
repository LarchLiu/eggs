FROM golang:1.25-bookworm AS builder

WORKDIR /src

COPY go.mod go.sum ./
RUN go mod download

COPY . .
RUN CGO_ENABLED=0 GOOS=linux GOARCH=amd64 go build -trimpath -ldflags="-s -w" -o /out/eggs-server .

FROM debian:bookworm-slim

RUN useradd --system --create-home --home-dir /app appuser

WORKDIR /app

COPY --from=builder /out/eggs-server /usr/local/bin/eggs-server

RUN mkdir -p /data && chown -R appuser:appuser /data /app

USER appuser

EXPOSE 8787

ENTRYPOINT ["eggs-server"]
CMD ["-addr", ":8787", "-data", "/data", "-base-url", "http://localhost:8787"]
