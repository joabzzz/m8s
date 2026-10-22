# Local control protocol clients

All four public call functions use one private exchange with one absolute
three-second connect/write/read deadline, starting before bounded request
serialization. The complete request plus newline must fit MAX_FRAME (65536 bytes)
before any connection. A bounded socket2 connect is followed by one nonblocking
owned Unix descriptor and safe readiness polling. No per-I/O timeout option
updates, reconnect, request replay or refreshed time budgets. Partial writes
preserve offsets; EINTR/WouldBlock consume the same deadline. Readiness HUP/error
is not success: only a complete bounded newline frame can proceed to the existing
response decoder. Buffered final frames can complete after peer close; partial
EOF/malformed/oversized responses refuse. First-frame/suffix behavior matches the
unchanged server read_frame helper.

A failed or timed-out reply does **not** prove a command had no effects: the server
may already have committed a mutation. Clients never automatically resend.
Errors use static stage labels and IO kinds, not raw paths/payload/error chains.
This transport does not add peer authentication, socket path trust or current-state
authority. Existing daemon checks and the dashboard's stronger path/peer checks
are separate and unchanged. This change is not a forensic explanation for past
unrelated transport failures.
