# Mobile message identity

Desktop 3.19.47 projects original server item IDs in live history, matching saved transcript IDs. Accepted steering uses serverUserMessageId; clientId remains a receipt alias. Mobile reconciles aliases before rendering. Identical text in distinct server messages remains distinct. Receipt confirmation still uses client request IDs.

Validation: desktop-bridge history tests; mobile saved/live merge regression, including stale client IDs retained before a saved snapshot arrives.
