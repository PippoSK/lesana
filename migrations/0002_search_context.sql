-- This is a derived search index; the official source text remains immutable.
CREATE FUNCTION reindex_legal_version(target_version bigint) RETURNS void
LANGUAGE sql AS $$
    WITH RECURSIVE ancestry AS (
        SELECT id, version_id, parent_anchor,
            CASE WHEN kind='group' THEN label ELSE heading END AS titles
        FROM legal_nodes WHERE version_id=target_version
        UNION ALL
        SELECT child.id, child.version_id, parent.parent_anchor,
            child.titles || ' ' || CASE WHEN parent.kind='group' THEN parent.label ELSE parent.heading END
        FROM ancestry child
        JOIN legal_nodes parent ON parent.version_id=child.version_id AND parent.anchor=child.parent_anchor
    )
    UPDATE legal_nodes n SET search_text=lower(unaccent(
        n.citation || ' ' || n.own_text || ' ' || ancestry.titles))
    FROM ancestry WHERE ancestry.id=n.id AND ancestry.parent_anchor IS NULL;
$$;

SELECT reindex_legal_version(id) FROM act_versions;
