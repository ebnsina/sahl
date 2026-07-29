-- Let enrollment consume its token before the device row exists.
--
-- Enrollment must claim the token *first*: the conditional UPDATE
-- (WHERE consumed_at IS NULL) is what makes two terminals racing to redeem the same token safe —
-- exactly one wins. But consumed_by references device(id), which is only created afterwards.
--
-- Deferring the check to commit satisfies both: the race stays closed, and the reference is still
-- verified before the transaction lands.
ALTER TABLE enrollment_token
    DROP CONSTRAINT enrollment_token_consumed_by_fk;

ALTER TABLE enrollment_token
    ADD CONSTRAINT enrollment_token_consumed_by_fk
    FOREIGN KEY (consumed_by) REFERENCES device (id) ON DELETE SET NULL
    DEFERRABLE INITIALLY DEFERRED;
