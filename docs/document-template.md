# Document Header Template

**Template Version**: 1.0 • **Last Updated**: December 11, 2025

Use this template for the header of all documentation pages.

---

```markdown
# Document Title

Brief one-paragraph description of what this document covers and who it's for.

**Version**: 0.1.0-beta.1 • **Last Updated**: YYYY-MM-DD

## Table of Contents

- [Section 1](#section-1)
- [Section 2](#section-2)
- [Subsection](#subsection)

## Section 1

Content starts here...
```

---

## Template Elements Explained

### Title (H1)
- **Clear and descriptive**: "API Reference" not "Functions"
- **Consistent naming**: Use standard terms from style guide
- **Sentence case**: "Error Reference" not "Error reference"

### Description Paragraph
- **One paragraph only**: Keep it brief (1-3 sentences)
- **Answer key questions**: What? Why? Who for?
- **Set expectations**: What readers will learn

**Examples**:
- ✅ "Complete reference for all public PostgreSQL functions exposed by pg_tviews."
- ✅ "Step-by-step guide to installing pg_tviews in different environments."
- ❌ "This document contains information about pg_tviews functions."

### Version and Date
- **Version**: Current pg_tviews version (0.1.0-beta.1)
- **Date**: YYYY-MM-DD format
- **Update on changes**: Keep current when modifying

### Table of Contents
- **Required for documents >500 lines**
- **Link to all H2 headings**
- **Include important H3 headings**
- **Keep updated**: Add new sections as added

---

## Complete Examples

### Reference Document

```markdown
# API Reference

Complete reference for all public PostgreSQL functions exposed by pg_tviews.

**Version**: 0.1.0-beta.1 • **Last Updated**: 2025-12-11

## Table of Contents

- [Extension Management](#extension-management)
- [DDL Operations](#ddl-operations)
- [Queue Management](#queue-management)
- [Debugging & Introspection](#debugging--introspection)
- [Two-Phase Commit](#two-phase-commit)
- [Manual Operations](#manual-operations)

## Extension Management

Content...
```

### Guide Document

```markdown
# Installation Guide

Complete installation instructions for pg_tviews in different environments.

**Version**: 0.1.0-beta.1 • **Last Updated**: 2025-12-11

## Table of Contents

- [System Requirements](#system-requirements)
- [Quick Install](#quick-install)
- [Platform-Specific Installation](#platform-specific-installation)
- [Post-Installation](#post-installation)
- [Troubleshooting](#troubleshooting)

## System Requirements

Content...
```

### Conceptual Document

```markdown
# Architecture Overview

High-level overview of pg_tviews internal architecture and design decisions.

**Version**: 0.1.0-beta.1 • **Last Updated**: 2025-12-11

## Table of Contents

- [Core Concepts](#core-concepts)
- [Data Flow](#data-flow)
- [Extension Components](#extension-components)
- [Performance Characteristics](#performance-characteristics)

## Core Concepts

Content...
```

---

## Guidelines

### When to Use Table of Contents

**Required**:
- Documents longer than 500 lines
- Documents with 3+ major sections
- Reference documents
- Multi-step guides

**Optional**:
- Short documents (<500 lines)
- Simple guides with 2-3 sections
- Documents that are primarily one long section

### Version Management

**Update version when**:
- Adding new features
- Changing existing functionality
- Major content reorganization

**Update date when**:
- Any content changes
- Even minor fixes or clarifications

### Title Guidelines

**Good titles**:
- API Reference
- Installation Guide
- Troubleshooting
- Performance Tuning

**Avoid**:
- Functions (too generic)
- How To Install (too verbose)
- Problems & Solutions (too vague)