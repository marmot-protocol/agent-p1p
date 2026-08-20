FROM rust:1.96.1-bookworm

ENV container=docker
ENV DEBIAN_FRONTEND=noninteractive

RUN apt-get update \
    && apt-get install -y --no-install-recommends dbus git jq systemd systemd-sysv util-linux \
    && apt-get clean \
    && rm -rf /var/lib/apt/lists/* \
    && systemctl mask dev-hugepages.mount sys-fs-fuse-connections.mount systemd-remount-fs.service

STOPSIGNAL SIGRTMIN+3
CMD ["/sbin/init"]
