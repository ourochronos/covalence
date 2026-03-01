FROM pgvector/pgvector:pg17

RUN apt-get update && apt-get install -y \
    build-essential \
    libreadline-dev \
    zlib1g-dev \
    flex \
    bison \
    git \
    postgresql-server-dev-17 \
    && rm -rf /var/lib/apt/lists/*

RUN git clone --branch release/PG17/1.7.0 --depth 1 https://github.com/apache/age.git /tmp/age \
    && cd /tmp/age \
    && make install \
    && rm -rf /tmp/age

COPY init-extensions.sql /docker-entrypoint-initdb.d/00-init-extensions.sql

RUN apt-get purge -y build-essential libreadline-dev zlib1g-dev flex bison git postgresql-server-dev-17 \
    && apt-get autoremove -y \
    && apt-get clean
