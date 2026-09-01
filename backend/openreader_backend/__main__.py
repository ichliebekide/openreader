import os

import uvicorn


def main() -> None:
    host = os.getenv("OPENREADER_HOST", "127.0.0.1")
    port = int(os.getenv("OPENREADER_PORT", "8765"))
    uvicorn.run("openreader_backend.main:app", host=host, port=port, log_level="info")


if __name__ == "__main__":
    main()
