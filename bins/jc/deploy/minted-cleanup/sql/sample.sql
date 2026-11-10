-- A fixed sample of real ids (outside the prefix): the ids whose md5 starts with 000, about one in
-- 4096. Taken before the first --apply (W/sample-before.tsv) and after each run; every line of the
-- first must still be in the later ones (LC_ALL=C comm -23 before after prints nothing).
BEGIN READ ONLY;
SELECT tenant_id, id FROM entities
 WHERE split_part(id, ':', 4) <> 'portal.smartcity.joinedcontext.com' AND left(md5(id), 3) = '000'
 ORDER BY 1, 2;
ROLLBACK;
