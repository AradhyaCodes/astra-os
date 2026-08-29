# Product

<!-- impeccable:product-schema 1 -->

## Platform

web

## Product Purpose

Aaru-OS is a Windows-hosted operating-system simulator. It makes operating-system concepts tangible through a graphical desktop backed by a real Rust system layer rather than a browser-only imitation.

## Positioning

The interface connects a persistent virtual filesystem, deterministic scheduler, simulated memory, process registry, security model, Almanac command language, and explicitly mounted host resources inside one inspectable desktop environment.

## Operating Context

Aaru runs as a Tauri desktop application on Windows. Users primarily operate through Almanac and inspect system behavior through graphical utilities. Host directories appear only after explicit mounting.

## Capabilities and Constraints

- Rust owns authentication, filesystem policy, process state, scheduling, memory simulation, host mounts, and command evaluation.
- React is a view and interaction layer over those backend contracts.
- Aaru virtual resources and physical host resources must remain visibly distinct.
- Host paths remain aliases unless the user explicitly inspects metadata.
- The product must never imply that Aaru encrypts or controls Windows files.
- Destructive host actions require stronger warnings than equivalent virtual actions.
- Metrics come from backend state or reasonable polling, never decorative randomness.

## Brand Commitments

Aaru-OS uses custom Aaru branding, a dark charcoal Windows-11-inspired desktop language that is not a direct copy, restrained translucent materials, and monospace only for commands and technical data. Almanac is intentionally more technical and retro than the surrounding desktop.

## Evidence on Hand

The repository contains the Aaru SVG mark and working Tauri commands for authentication, virtual and host filesystems, Almanac, host shell streaming, applications, processes, scheduling, and simulated memory.

## Product Principles

- System truth comes from Rust.
- Make virtual-versus-host boundaries unmistakable.
- Almanac remains the primary launcher and power-user surface.
- Prefer calm, direct operational feedback over decorative activity.
- Treat physical-computer operations with proportional caution.
