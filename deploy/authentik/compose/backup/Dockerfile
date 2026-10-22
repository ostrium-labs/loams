# pg_dump 17 + rclone in one small image. Both are pinned by digest.
FROM docker.io/rclone/rclone:1.75.1@sha256:45401ad7410db1d67ffdb58e19059ad20b0d8e0285a60e38bbec55cc1019c7a5 AS rclone

FROM docker.io/library/postgres:17-alpine@sha256:b0f9560a2de083e2cc7382e75f808c7381a32852a7ec49117deedb300e552b24
COPY --from=rclone /usr/local/bin/rclone /usr/local/bin/rclone
COPY backup.sh restore.sh /usr/local/bin/
RUN chmod 0755 /usr/local/bin/backup.sh /usr/local/bin/restore.sh && mkdir -p /scratch
# postgres:alpine runs as root unless told otherwise; the entrypoint of the
# base image is bypassed because this container never starts a server.
ENTRYPOINT []
CMD ["/usr/local/bin/backup.sh"]
