#!/usr/bin/env python3
"""Read-only S3 usage consistency checks for a closed, historical range.

This script never creates a skill, sends a message, or starts a run. A
successful result is an API-local check only; it is not full S3 acceptance.
"""

from __future__ import annotations

import argparse
import datetime as dt
import math
import sys
from typing import Any, Iterable

HERE = __import__("pathlib").Path(__file__).resolve()
sys.path.insert(0, str(HERE.parents[1]))

from common import (  # noqa: E402
    PollTimeout,
    RpcError,
    RpcTransportError,
    add_connection_args,
    client_from_args,
    json_dump,
    ready_health,
    require_dict,
    require_list,
    require_production_host,
    safe_error,
)


USAGE_INT_FIELDS = (
    "input_tokens",
    "output_tokens",
    "cache_read_tokens",
    "cache_write_tokens",
    "requests",
)


class EmptyUsage(ValueError):
    """The requested closed range has no requests to validate."""


def args_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    add_connection_args(parser)
    parser.add_argument("--from", dest="from_time", required=True, help="RFC 3339 start, including timezone")
    parser.add_argument("--to", dest="to_time", required=True, help="RFC 3339 end, including timezone")
    return parser


def parse_closed_time(value: str, label: str) -> dt.datetime:
    if not isinstance(value, str) or "T" not in value:
        raise ValueError(f"--{label} must be a complete RFC 3339 timestamp with timezone")
    try:
        parsed = dt.datetime.fromisoformat(value.replace("Z", "+00:00"))
    except ValueError as exc:
        raise ValueError(f"--{label} is not a valid RFC 3339 timestamp") from exc
    if parsed.tzinfo is None or parsed.utcoffset() is None:
        raise ValueError(f"--{label} must include an explicit timezone")
    return parsed


def validate_range(args: argparse.Namespace) -> tuple[dt.datetime, dt.datetime]:
    start = parse_closed_time(args.from_time, "from")
    end = parse_closed_time(args.to_time, "to")
    if end <= start:
        raise ValueError("--to must be after --from")
    if end > dt.datetime.now(dt.timezone.utc):
        raise ValueError("usage range must be closed in the past")
    return start, end


def is_number(value: Any) -> bool:
    return isinstance(value, (int, float)) and not isinstance(value, bool)


def require_integer(value: Any, label: str) -> int:
    if not isinstance(value, int) or isinstance(value, bool):
        raise ValueError(f"{label} must be an integer")
    return value


def validate_cost(value: Any, label: str) -> None:
    if value is not None and not is_number(value):
        raise ValueError(f"{label} must be numeric or null")


def validate_usage_totals(value: Any, label: str) -> dict[str, Any]:
    usage = require_dict(value, label)
    for field in USAGE_INT_FIELDS:
        require_integer(usage.get(field), f"{label}.{field}")
    validate_cost(usage.get("cost"), f"{label}.cost")
    return usage


def sum_ints(items: Iterable[dict[str, Any]], field: str) -> int:
    return sum(require_integer(item.get(field), f"aggregate {field}") for item in items)


def validate_heatmap(value: Any) -> dict[str, Any]:
    heatmap = require_dict(value, "usage.heatmap result")
    days = require_list(heatmap.get("days"), "usage.heatmap.days")
    thresholds = require_list(heatmap.get("thresholds"), "usage.heatmap.thresholds")
    if len(thresholds) != 3 or any(not is_number(item) for item in thresholds):
        raise ValueError("usage.heatmap.thresholds must contain three numbers")
    checked: list[dict[str, Any]] = []
    for index, raw in enumerate(days):
        day = require_dict(raw, f"usage.heatmap.days[{index}]")
        if not isinstance(day.get("date"), str):
            raise ValueError("usage.heatmap day.date must be a string")
        for field in ("value", "tokens", "requests"):
            if not is_number(day.get(field)):
                raise ValueError(f"usage.heatmap day.{field} must be numeric")
        validate_cost(day.get("cost"), f"usage.heatmap day {index}.cost")
        if day.get("top_bot_id") is not None and not isinstance(day.get("top_bot_id"), str):
            raise ValueError("usage.heatmap day.top_bot_id must be string or null")
        checked.append(day)
    return {"days": checked, "thresholds": thresholds}


def validate_timeseries(value: Any) -> dict[str, Any]:
    timeseries = require_dict(value, "usage.timeseries result")
    granularity = timeseries.get("granularity")
    if granularity not in {"hour", "day", "week"}:
        raise ValueError("usage.timeseries.granularity is invalid")
    buckets = require_list(timeseries.get("buckets"), "usage.timeseries.buckets")
    raw_series = require_list(timeseries.get("series"), "usage.timeseries.series")
    series: list[dict[str, Any]] = []
    for index, raw in enumerate(raw_series):
        item = require_dict(raw, f"usage.timeseries.series[{index}]")
        values = require_list(item.get("values"), f"usage.timeseries.series[{index}].values")
        input_values = require_list(item.get("input_values"), f"usage.timeseries.series[{index}].input_values")
        output_values = require_list(item.get("output_values"), f"usage.timeseries.series[{index}].output_values")
        if not all(len(values) == len(array) == len(buckets) for array in (input_values, output_values)):
            raise ValueError("usage.timeseries value arrays must match buckets")
        for field, array in (("values", values), ("input_values", input_values), ("output_values", output_values)):
            if any(not is_number(number) for number in array):
                raise ValueError(f"usage.timeseries {field} contains a non-numeric value")
        total = item.get("total")
        if not is_number(total) or sum(values) != total:
            raise ValueError("usage.timeseries series.total does not equal series.values")
        if sum(input_values) + sum(output_values) != total:
            raise ValueError("usage.timeseries input/output totals do not equal series.total")
        series.append({
            "key": item.get("key"),
            "label": item.get("label"),
            "total": total,
            "values": values,
            "input_values": input_values,
            "output_values": output_values,
        })
    return {"granularity": granularity, "buckets": buckets, "series": series}


def validate_breakdown(value: Any) -> dict[str, Any]:
    breakdown = require_dict(value, "usage.breakdown result")
    raw_rows = require_list(breakdown.get("rows"), "usage.breakdown.rows")
    rows: list[dict[str, Any]] = []
    for index, raw in enumerate(raw_rows):
        row = require_dict(raw, f"usage.breakdown.rows[{index}]")
        usage = validate_usage_totals(row.get("usage"), f"usage.breakdown.rows[{index}].usage")
        sparkline = require_list(row.get("sparkline"), f"usage.breakdown.rows[{index}].sparkline")
        if any(not is_number(number) for number in sparkline):
            raise ValueError("usage.breakdown.sparkline contains a non-numeric value")
        phases = require_dict(row.get("phases"), f"usage.breakdown.rows[{index}].phases")
        rows.append({
            "key": row.get("key"),
            "label": row.get("label"),
            "usage": usage,
            "sparkline_length": len(sparkline),
            "phase_keys": sorted(phases),
        })
    return {"rows": rows}


def compare_summary(summary: dict[str, Any]) -> tuple[dict[str, Any], dict[str, Any]]:
    current = validate_usage_totals(summary.get("current"), "usage.summary.current")
    previous = validate_usage_totals(summary.get("previous"), "usage.summary.previous")
    if current.get("requests", 0) <= 0:
        raise EmptyUsage("EMPTY: usage.summary.current.requests is zero for the requested range")
    return current, previous


def scenario(args: argparse.Namespace) -> dict[str, Any]:
    start, end = validate_range(args)
    client = client_from_args(args)
    health = ready_health(client, args)
    require_production_host(client, health)
    params = {"from": args.from_time, "to": args.to_time}
    summary_raw = require_dict(client.call("usage.summary", params), "usage.summary result")
    current, previous = compare_summary(summary_raw)
    heatmap = validate_heatmap(client.call("usage.heatmap", {**params, "mode": "calendar", "metric": "tokens"}))
    timeseries = validate_timeseries(client.call(
        "usage.timeseries",
        {**params, "granularity": "auto", "dimension": "bot", "metric": "tokens", "split_io": True, "top": 100},
    ))
    breakdown = validate_breakdown(client.call("usage.breakdown", {**params, "dimension": "bot"}))

    summary_io = current["input_tokens"] + current["output_tokens"]
    heatmap_tokens = sum(int(day["tokens"]) for day in heatmap["days"])
    heatmap_requests = sum(int(day["requests"]) for day in heatmap["days"])
    series = timeseries["series"]
    series_total = sum(item["total"] for item in series)
    series_values = sum(sum(item["values"]) for item in series)
    series_input = sum(sum(item["input_values"]) for item in series)
    series_output = sum(sum(item["output_values"]) for item in series)
    rows = [item["usage"] for item in breakdown["rows"]]
    breakdown_totals = {field: sum_ints(rows, field) for field in USAGE_INT_FIELDS}
    breakdown_costs = [row.get("cost") for row in rows]
    heatmap_costs = [day.get("cost") for day in heatmap["days"]]
    all_costs = breakdown_costs + heatmap_costs

    numeric_costs = [cost for cost in all_costs if cost is not None]
    null_cost_count = len(all_costs) - len(numeric_costs)
    if current.get("cost") is not None:
        if null_cost_count:
            raise ValueError("usage summary has a numeric cost but a detail aggregate is unknown")
        if not math.isclose(sum(breakdown_costs), current["cost"], rel_tol=1e-9, abs_tol=1e-12):
            raise ValueError("usage.breakdown cost total does not match usage.summary")
        if not math.isclose(sum(heatmap_costs), current["cost"], rel_tol=1e-9, abs_tol=1e-12):
            raise ValueError("usage.heatmap cost total does not match usage.summary")
        cost_status = "known_and_matching"
    else:
        # The server's aggregate cost is null when any record in that
        # aggregate has no price. Other Bot/day aggregates can still have a
        # known numeric cost, so a null summary must not reject mixed detail.
        cost_status = "unknown_with_mixed_detail" if numeric_costs else "unknown_all_detail_null"

    breakdown_matches = all(breakdown_totals[field] == current[field] for field in USAGE_INT_FIELDS)
    if not breakdown_matches:
        raise ValueError("usage.breakdown totals do not match usage.summary.current")
    if heatmap_tokens != summary_io or heatmap_requests != current["requests"]:
        raise ValueError("usage.heatmap totals do not match usage.summary.current")
    if series_values != series_total or series_input + series_output != series_total:
        raise ValueError("usage.timeseries internal totals are inconsistent")
    top_truncation_possible = len(series) >= 100
    if not top_truncation_possible and series_total != summary_io:
        raise ValueError("usage.timeseries totals do not match usage.summary.current")
    timeseries_compare = "unknown_top_truncation" if top_truncation_possible else "exact"
    overall_status = "PARTIAL" if top_truncation_possible else "PASS"

    return {
        "scenario": "S3 usage read-only consistency",
        "status": overall_status,
        "api_local_pass": overall_status == "PASS",
        "full_s3_pass": False,
        "url": client.base_url,
        "health_version": health.get("version"),
        "range": {
            "from": args.from_time,
            "to": args.to_time,
            "from_utc": start.astimezone(dt.timezone.utc).isoformat(),
            "to_utc": end.astimezone(dt.timezone.utc).isoformat(),
            "closed_in_past": True,
        },
        "summary": {"current": current, "previous": previous},
        "checks": {
            "summary_io_tokens": summary_io,
            "heatmap": {
                "days": len(heatmap["days"]),
                "thresholds": heatmap["thresholds"],
                "tokens": heatmap_tokens,
                "requests": heatmap_requests,
                "matches_summary": True,
            },
            "timeseries": {
                "granularity": timeseries["granularity"],
                "buckets": len(timeseries["buckets"]),
                "series": len(series),
                "series_total": series_total,
                "values_total": series_values,
                "input_total": series_input,
                "output_total": series_output,
                "comparison": timeseries_compare,
                "top": 100,
                "top_truncation_possible": top_truncation_possible,
            },
            "breakdown": {
                "rows": len(rows),
                "totals": breakdown_totals,
                "matches_summary": breakdown_matches,
                "cost_status": cost_status,
                "detail_numeric_cost_count": len(numeric_costs),
                "detail_unknown_cost_count": null_cost_count,
                "known_cost_sum": sum(numeric_costs) if numeric_costs else None,
            },
        },
        "note": "API checks only; dashboard parity across desktop/Android and the remaining S3 skill/memory checks remain manual.",
    }


def main(args: argparse.Namespace) -> int:
    try:
        result = scenario(args)
        code = 0 if result.get("status") == "PASS" else 2
    except EmptyUsage as exc:
        result = {
            "scenario": "S3 usage read-only consistency",
            "status": "EMPTY",
            "api_local_pass": False,
            "full_s3_pass": False,
            "error": safe_error(args, exc),
        }
        code = 2
    except (RpcError, RpcTransportError, PollTimeout, ValueError) as exc:
        result = {
            "scenario": "S3 usage read-only consistency",
            "status": "FAIL",
            "api_local_pass": False,
            "full_s3_pass": False,
            "error": safe_error(args, exc),
        }
        code = 1
    if args.json:
        print(json_dump(result))
    elif result["status"] == "PASS":
        print(f"API checks: PASS {result['scenario']} (API local; full_s3_pass=false)")
    elif result["status"] == "PARTIAL":
        print(f"API checks: PARTIAL {result['scenario']} (timeseries top=100 may be truncated)", file=sys.stderr)
    else:
        print(f"API checks: {result['status']}: {result.get('error', 'usage check did not pass')}", file=sys.stderr)
    return code


if __name__ == "__main__":
    raise SystemExit(main(args_parser().parse_args()))
