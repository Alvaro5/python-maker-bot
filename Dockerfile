# Use a lightweight Python image
FROM python:3.12-slim

# Install bash (for venv setup scripts) and ensure venv module is available
RUN apt-get update && apt-get install -y --no-install-recommends bash \
    && rm -rf /var/lib/apt/lists/*

# Pre-bake common data science and utility libraries so they don't need to be
# installed on every ephemeral container execution. This dramatically speeds up
# runs that depend on popular packages.
RUN pip install --no-cache-dir \
    numpy \
    pandas \
    matplotlib \
    scikit-learn \
    scipy \
    requests \
    flask \
    pygame \
    Pillow

# Set a non-root user for security
RUN useradd -m sandboxuser

# Create the scripts mount point
RUN mkdir -p /home/sandboxuser/scripts && chown sandboxuser:sandboxuser /home/sandboxuser/scripts

USER sandboxuser
WORKDIR /home/sandboxuser

# Default command — overridden by the Rust code at runtime
CMD ["python3", "scripts/script.py"]