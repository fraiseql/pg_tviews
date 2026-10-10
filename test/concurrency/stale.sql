-- Rows of each TVIEW that differ from its backing view (missing, extra or stale).
SELECT (SELECT count(*) FROM tv_user t FULL JOIN tviews.public__tv_user v USING (pk_user)
         WHERE t.data IS DISTINCT FROM v.data)
     + (SELECT count(*) FROM tv_post t FULL JOIN tviews.public__tv_post v USING (pk_post)
         WHERE t.data IS DISTINCT FROM v.data)
     + (SELECT count(*) FROM tv_note t FULL JOIN tviews.public__tv_note v USING (pk_note)
         WHERE t.data IS DISTINCT FROM v.data);
