# Use a lightweight Python image
FROM python:3.11-slim

# Set a non-root user for security
RUN useradd -m sandboxuser

# Create the scripts mount point
RUN mkdir -p /home/sandboxuser/scripts && chown sandboxuser:sandboxuser /home/sandboxuser/scripts

USER sandboxuser
WORKDIR /home/sandboxuser

# Default command — overridden by the Rust code at runtime
CMD ["python3", "scripts/script.py"]