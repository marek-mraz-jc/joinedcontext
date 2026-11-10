-- Rows outside the prefix whose document names a copy, per tenant. The cleanup account holds
-- deleteBatch only, so the gateway's HeldDelete refuses a copy such a row points at and the run
-- stops on it. Expect no line; run it right before each --apply.
BEGIN READ ONLY;
SELECT tenant_id, count(*) FROM entities
 WHERE split_part(id, ':', 4) <> 'portal.smartcity.joinedcontext.com'
   AND entity::text LIKE '%:portal.smartcity.joinedcontext.com:%'
 GROUP BY 1 ORDER BY 1;
ROLLBACK;
