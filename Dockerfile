# syntax=docker/dockerfile:1
#
# Asterisk 22.10.1 (aistack/main) container build.
#
# Two stages:
#   builder  - compiles Asterisk from the source tree in this repo, using
#              the bundled pjproject and bundled jansson (exactly the flags
#              this tree is known to build clean with on the host:
#              `./configure --with-pjproject-bundled --with-jansson-bundled`).
#   runtime  - copies the installed tree out of the builder and ships only
#              the runtime shared libraries Asterisk actually links against
#              (verified with ldd against the compiled binaries/modules).

ARG BASE_IMAGE=ubuntu:24.04

########################################
# Stage 1: builder
########################################
FROM ${BASE_IMAGE} AS builder

ENV DEBIAN_FRONTEND=noninteractive

# Build-time dependencies. This is the exact set the source tree is known
# to configure/build against with zero warnings, plus the tools bundled
# pjproject/jansson need to download and unpack their own tarballs
# (wget, file) and the toolchain to build them (build-essential, pkg-config).
RUN apt-get update && apt-get install -y --no-install-recommends \
        build-essential \
        libedit-dev \
        uuid-dev \
        libxml2-dev \
        libsqlite3-dev \
        libjansson-dev \
        libssl-dev \
        libsrtp2-dev \
        pkg-config \
        libncurses-dev \
        wget \
        curl \
        ca-certificates \
        file \
        git \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /usr/src/asterisk

# Copy the source tree. The host tree was built in place (config.log,
# menuselect.makeopts, *.o, *.so, etc. are all present from the native
# build), so before configuring for the container we blow away any
# previously generated build state with `make distclean`. This guarantees
# a fully fresh ./configure + make inside the container rather than
# reusing host-built objects (which would be built for a possibly
# different libc/toolchain).
COPY . .
RUN make distclean || true

RUN ./configure --with-pjproject-bundled --with-jansson-bundled

# Build with all available cores.
RUN make -j"$(nproc)"

# Install into a staging root we'll copy into the runtime image.
RUN make install DESTDIR=/opt/asterisk-install

# Create the runtime directory skeleton (log/spool/lib dirs, astdb, etc.)
# the same way `make config` / packaging would, without pulling in the
# full interactive sample configs -- we ship our own minimal config set
# at runtime via docker/etc-asterisk. `make install` above already
# creates /etc/asterisk (empty) and the var directories under DESTDIR.
#
# var/run is deliberately NOT created/kept here: on Debian/Ubuntu, /var/run
# is a symlink to /run in the base image, but `make install`'s bininstall
# target creates DESTDIR/var/run/asterisk as a real directory. Copying a
# real directory over a symlink path in the runtime stage's
# `COPY --from=builder` fails ("cannot copy to non-directory: .../var/run"),
# so we drop it here and let the runtime stage's own `mkdir -p
# /var/run/asterisk` create it through the live symlink instead.
RUN rm -rf /opt/asterisk-install/var/run \
    && mkdir -p \
        /opt/asterisk-install/var/lib/asterisk \
        /opt/asterisk-install/var/log/asterisk \
        /opt/asterisk-install/var/log/asterisk/cdr-csv \
        /opt/asterisk-install/var/spool/asterisk \
        /opt/asterisk-install/var/spool/asterisk/voicemail \
        /opt/asterisk-install/etc/asterisk

# Sanity check: list the shared library dependencies of the main binary
# and all modules so we know exactly what the runtime stage needs to
# install. (Shown in `docker build` logs; also useful for future upgrades.)
RUN echo "== asterisk binary deps ==" \
    && ldd /opt/asterisk-install/usr/sbin/asterisk \
    && echo "== module deps (unique libs) ==" \
    && ( for f in /opt/asterisk-install/usr/lib/*/asterisk/modules/*.so \
                  /opt/asterisk-install/usr/lib/asterisk/modules/*.so; do \
           [ -e "$f" ] && ldd "$f" 2>/dev/null; \
         done | awk '{print $1}' | sort -u )

########################################
# Stage 2: runtime
########################################
FROM ${BASE_IMAGE} AS runtime

ENV DEBIAN_FRONTEND=noninteractive

# Runtime-only shared libraries. Bundled pjproject (libasteriskpj) and
# bundled jansson are statically linked into Asterisk's own bundled
# libraries at build time, so libjansson4/libpjproject are NOT needed
# here -- confirmed with ldd in the builder stage (no libjansson*.so or
# libpj*.so shows up as a dynamic dependency). libsrtp2 IS a real runtime
# dependency: res_srtp.so links libsrtp2.so.1 dynamically.
RUN apt-get update && apt-get install -y --no-install-recommends \
        libedit2 \
        libxml2 \
        libsqlite3-0 \
        libssl3 \
        libsrtp2-1 \
        libuuid1 \
        libtinfo6 \
        ca-certificates \
        wget \
    && rm -rf /var/lib/apt/lists/*

# Non-root user to run Asterisk as. Asterisk supports dropping privileges
# via runuser/rungroup in asterisk.conf; we create matching uid/gid here
# and chown the writable data directories so that works out of the box.
RUN groupadd -r asterisk && useradd -r -g asterisk -d /var/lib/asterisk -s /usr/sbin/nologin asterisk

# Bring in the full installed tree from the builder (binaries, libs,
# modules, sounds, default /etc/asterisk skeleton, var directories).
COPY --from=builder /opt/asterisk-install/ /

RUN ldconfig \
    && mkdir -p /var/lib/asterisk /var/log/asterisk /var/log/asterisk/cdr-csv \
               /var/spool/asterisk /var/spool/asterisk/voicemail /var/run/asterisk /etc/asterisk \
    && chown -R asterisk:asterisk /var/lib/asterisk /var/log/asterisk /var/spool/asterisk /var/run/asterisk /etc/asterisk

USER asterisk

EXPOSE 5060/udp 5060/tcp 8088/tcp 8089/tcp 10000-10100/udp

# Foreground, verbose, container-friendly (no daemonizing, logs to stdout
# via the console as well as logger.conf targets).
ENTRYPOINT ["/usr/sbin/asterisk", "-f", "-vvv"]
