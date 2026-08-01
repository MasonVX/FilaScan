FROM python:3.13-alpine

WORKDIR /app
COPY spoolman_bridge.py /app/spoolman_bridge.py

USER nobody
ENTRYPOINT ["python", "/app/spoolman_bridge.py"]
