INVITE sip:bob@example.com SIP/2.0
Via: SIP/2.0/UDP peer.invalid;branch=fixture-cancel-1
From: Alice <sip:alice@example.com>;tag=alice-cancel
To: Bob <sip:bob@example.com>
Call-ID: fixture-cancel-call
CSeq: 1 INVITE
Contact: <sip:alice@127.0.0.1:5060>
Content-Length: 0
