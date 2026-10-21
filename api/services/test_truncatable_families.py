"""A family that grows must be truncated, not refused.

`list_documents` has had a mode that returns what fits since the retention
sweep needed it, and no caller ever passed it -- twenty-nine call sites, none
of them. So the mode existed, was tested in isolation, and never ran. Every
read of query history, activity or chat still answered 503 the moment an owner
passed the listing bound.

These tests are about the default rather than the mechanism: that a family
which grows is truncated without being asked, that one which does not still
refuses, and that a caller can still say which it wants.
"""
import unittest
from unittest import mock

from fastapi import HTTPException

from services import product_read_authority as authority


def _records(count: int, owner: str = "person@example.test") -> list[dict]:
    return [
        {"document": {"id": f"record-{index}", "user_email": owner, "created_by": owner}}
        for index in range(count)
    ]


class TruncatableFamilies(unittest.TestCase):
    """What happens when a family runs past the listing bound."""

    def setUp(self) -> None:
        self.actor = "person@example.test"
        patch = mock.patch.object(authority, "enabled", return_value=True)
        patch.start()
        self.addCleanup(patch.stop)
        # `gather` runs the reads it is given; calling them directly keeps the
        # test about the listing choice rather than about concurrency.
        gather = mock.patch.object(
            authority.product_reads, "gather",
            side_effect=lambda *reads: [read() for read in reads],
        )
        gather.start()
        self.addCleanup(gather.stop)

    def test_a_family_that_grows_is_truncated_rather_than_refused(self) -> None:
        """Query history past the bound returns rows instead of 503."""
        for family in ("query_history", "activity", "user_recents"):
            with self.subTest(family=family):
                with mock.patch.object(
                    authority.product_store, "list_records",
                    side_effect=AssertionError(
                        f"{family} must not take the refusing listing path"),
                ), mock.patch.object(
                    authority.product_store, "list_records_capped",
                    return_value=(_records(5, self.actor), True),
                ):
                    documents = authority.list_documents(family, self.actor, "Admin")
                self.assertEqual(len(documents), 5)

    def test_a_curated_family_still_refuses(self) -> None:
        """A dataset missing from a dataset list is a fault, not a truncation."""
        with mock.patch.object(
            authority.product_store, "list_records",
            side_effect=HTTPException(503, "KaveonDB product list exceeds its configured bound"),
        ), mock.patch.object(
            authority.product_store, "list_records_capped",
            side_effect=AssertionError("datasets must not be silently truncated"),
        ):
            with self.assertRaises(HTTPException) as raised:
                authority.list_documents("datasets", self.actor, "Admin")
        self.assertEqual(raised.exception.status_code, 503)

    def test_a_caller_can_still_choose(self) -> None:
        """The family sets the default; the caller may override it."""
        # A curated family, asked for explicitly as capped. Datasets also
        # side-read favourites through the uncapped listing, which is correct
        # and not what is under test, so only the family read is refused here.
        def only_favourites(kind, *_args, **_kwargs):
            if kind != "favorite":
                raise AssertionError("an explicit cap must be honoured")
            return []

        with mock.patch.object(
            authority.product_store, "list_records", side_effect=only_favourites,
        ), mock.patch.object(
            authority.product_store, "list_records_capped",
            return_value=(_records(2, self.actor), True),
        ):
            self.assertEqual(
                len(authority.list_documents(
                    "datasets", self.actor, "Admin", capped=True)),
                2,
            )

        # A growing family, asked for explicitly as whole.
        with mock.patch.object(
            authority.product_store, "list_records",
            return_value=_records(3, self.actor),
        ), mock.patch.object(
            authority.product_store, "list_records_capped",
            side_effect=AssertionError("an explicit refusal must be honoured"),
        ):
            self.assertEqual(
                len(authority.list_documents(
                    "query_history", self.actor, "Admin", capped=False)),
                3,
            )

    def test_every_truncatable_family_is_a_real_family(self) -> None:
        """A typo here would silently restore the 503 it was meant to remove."""
        for family in authority._TRUNCATABLE_FAMILIES:
            with self.subTest(family=family):
                self.assertIn(
                    family,
                    set(authority._KIND) | {"chat_history"},
                    f"'{family}' is not a family, so nothing will ever match it",
                )


if __name__ == "__main__":
    unittest.main()
