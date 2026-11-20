"""Paired SPEC-573 timing fixtures, in seconds. Run with Python 3.11 or later.

All jobs in the timing model pass, take one slot and have equal priority.
The adjacent Rust test exercises real processes, failed checks and missing output.
"""
from collections import deque
from dataclasses import dataclass, field
from statistics import mean, median


@dataclass
class Gate:
    arrival: int
    durations: tuple[int, ...]
    admitted: bool = False
    passed: set[int] = field(default_factory=set)
    finished: int | None = None


@dataclass
class Result:
    twice: int
    passed_twice: int
    repeated_pass_seconds: int
    all_second_seconds: int
    gate_elapsed: tuple[int, ...]
    last_gate: int
    ready_to_serving: int


def simulate(work, slots, ready=None, policy="current", grace=30, stop_lag=25):
    gates = [Gate(arrival, tuple(durations)) for arrival, durations in work]
    running = {}  # (run id, job ordinal) -> first second of this attempt
    queue = deque()
    first_attempts = {}
    duplicates = repeated_passes = repeated_pass_seconds = all_second_seconds = 0
    helper_request = stop = serving = None
    hold_new_runs = False

    # One restart at most. This serial-work upper bound also covers delayed arrivals.
    limit = max(g.arrival for g in gates) + 2 * sum(sum(g.durations) for g in gates)
    limit += (ready or 0) + grace + 39 + 4
    for now in range(limit + 1):
        for job_id, started in list(running.items()):
            run, job = job_id
            gate = gates[run]
            if now == started + gate.durations[job]:
                del running[job_id]
                gate.passed.add(job)
                if job_id in first_attempts:
                    duplicates += 1
                    all_second_seconds += now - started
                    if first_attempts[job_id] == "passed":
                        repeated_passes += 1
                        repeated_pass_seconds += now - started
                else:
                    first_attempts[job_id] = "passed"
                if len(gate.passed) == len(gate.durations):
                    gate.finished = now

        if now == ready and policy == "all runs":
            hold_new_runs = True
        if ready is not None and now >= ready + 4 and (now - ready) % 2 == 0 and helper_request is None:
            if policy == "all runs":
                wait = any(g.admitted and g.finished is None for g in gates)
            elif policy == "last job":
                wait = any(gates[run].passed and len(gates[run].passed) + 1 == len(gates[run].durations)
                           for run, _ in running)
            else:
                wait = False
            if not wait or now >= ready + grace:
                helper_request, stop, serving = now, now + stop_lag, now + 39

        if now == stop:
            for job_id in running:
                first_attempts[job_id] = "interrupted"
            running.clear()
            queue.clear()
            for gate in gates:
                if gate.finished is None:
                    gate.admitted = False
                    # Upper bound for hypothetical jobs with proved immutable inputs and outputs.
                    if policy != "valid passes":
                        gate.passed.clear()
            hold_new_runs = True
        if now == serving:
            hold_new_runs = False
        if not hold_new_runs:
            for run, gate in enumerate(gates):
                if gate.arrival <= now and gate.finished is None and not gate.admitted:
                    gate.admitted = True
                    queue.extend((run, job) for job in range(len(gate.durations)) if job not in gate.passed)
        while queue and len(running) < slots:
            running[queue.popleft()] = now

        update_done = ready is None or (serving is not None and now >= serving)
        if update_done and all(g.finished is not None for g in gates):
            return Result(duplicates, repeated_passes, repeated_pass_seconds, all_second_seconds,
                          tuple(g.finished - g.arrival for g in gates), max(g.finished for g in gates),
                          0 if ready is None else serving - ready)
    raise AssertionError("fixture did not finish within its serial-work upper bound")


def sweep(cases, slots, policy, grace, stop_lag):
    pairs = []
    for work in cases:
        ordinary = simulate(work, slots)
        for ready in range(0, ordinary.last_gate, 5):
            control = simulate(work, slots, ready, stop_lag=stop_lag)
            candidate = simulate(work, slots, ready, policy, grace, stop_lag)
            pairs.append((control, candidate))
    averages = {}
    for name in ("twice", "passed_twice", "repeated_pass_seconds", "all_second_seconds", "last_gate", "ready_to_serving"):
        averages[name] = tuple(round(mean(getattr(pair[i], name) for pair in pairs), 2) for i in (0, 1))
    print(policy, "cap", grace, "stop lag", stop_lag, "pairs", len(pairs), averages,
          "median gate delta", median(b.last_gate - a.last_gate for a, b in pairs),
          "worst added wait", max(b.ready_to_serving - a.ready_to_serving for a, b in pairs))


if __name__ == "__main__":
    fixtures = [
        ("ordinary", [(0, [30, 55]), (20, [60])], None, 3),
        ("tail", [(0, [35, 68, 113])], 68, 3),
        ("mixed", [(0, [35, 65, 112]), (0, [50, 270]), (90, [60])], 68, 5),
        ("long", [(0, [40, 270])], 75, 2),
        ("empty", [(130, [25])], 75, 2),
        ("saturated", [(0, [100, 150, 220, 320]), (5, [115, 130]),
                       (20, [40, 180]), (90, [20, 160])], 75, 4),
    ]
    for label, work, ready, slots in fixtures:
        for policy in ("current", "all runs", "last job", "valid passes"):
            print(label, policy, simulate(work, slots, ready, policy, 60 if policy == "all runs" else 30))

    shapes = [[248, 318, 315, 392], [552, 447, 436, 437], [267, 344, 346, 419], [334, 282, 210, 396]]
    for label, cases, slots in [("solo", [[(0, jobs)] for jobs in shapes], 4),
                                ("mixed", [list(zip([0, 45, 200, 360], shapes))], 6)]:
        print(label)
        for policy, bound, lag in [("valid passes", 0, 25), ("all runs", 60, 25), ("last job", 15, 25),
                                   ("last job", 30, 25), ("last job", 60, 25), ("last job", 30, 0),
                                   ("last job", 30, 38)]:
            sweep(cases, slots, policy, bound, lag)
