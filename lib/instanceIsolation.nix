# Agent-owned state must never alias or contain another managed instance's data.
{ lib }:
entries:
let
  within = parent: child: child == parent || lib.hasPrefix "${parent}/" child;
  roots =
    entry:
    lib.unique [
      entry.stateDir
      entry.stateStorage
      entry.workspace
    ];
  separate =
    left: right:
    left.container == right.container
    || lib.all (a: lib.all (b: !(within a b || within b a)) (roots right)) (roots left);
in
lib.all (left: lib.all (right: separate left right) entries) entries
