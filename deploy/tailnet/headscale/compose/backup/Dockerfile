# sqlite3 + age + rclone in one small image. Pinned by digest.
FROM docker.io/rclone/rclone:1.75.1@sha256:45401ad7410db1d67ffdb58e19059ad20b0d8e0285a60e38bbec55cc1019c7a5 AS rclone

FROM docker.io/library/alpine:3.24@sha256:294b683cb724975bec92580e1e685676bd4b50bda910ddb8c51d4cabeaec77e6
RUN apk add --no-cache sqlite age tar
COPY --from=rclone /usr/local/bin/rclone /usr/local/bin/rclone
COPY backup.sh restore.sh /usr/local/bin/
RUN chmod 0755 /usr/local/bin/backup.sh /usr/local/bin/restore.sh && mkdir -p /scratch
CMD ["/usr/local/bin/backup.sh"]
