# Post-Incident Review Runbook

## Purpose
Conduct thorough analysis of incidents to identify root causes, improve processes, and prevent future occurrences.

## When to Use
- **After Any Incident**: All incidents require review, regardless of severity
- **Major Incidents**: SEV 1 and SEV 2 incidents require detailed analysis
- **Recurring Issues**: When similar incidents happen multiple times
- **Process Failures**: When incident response processes broke down
- **Learning Opportunities**: Even successful resolutions can provide insights

## Prerequisites
- Incident documentation complete
- All participants available for discussion
- Timeline of events reconstructed
- Metrics and monitoring data collected
- Access to relevant logs and system data

## Phase 1: Review Preparation (30 minutes)

### Step 1: Timeline Reconstruction
```sql
-- Gather incident timeline data
CREATE TEMP TABLE incident_timeline AS
SELECT
    'Incident Start' as event,
    incident_start_time as timestamp,
    'User reported issue' as description
UNION ALL
SELECT
    'Detection' as event,
    detection_time as timestamp,
    'Monitoring alert or user report' as description
UNION ALL
SELECT
    'Response Start' as event,
    response_start_time as timestamp,
    'Team began incident response' as description
UNION ALL
SELECT
    'Containment' as event,
    containment_time as timestamp,
    'Issue contained, impact limited' as description
UNION ALL
SELECT
    'Resolution' as event,
    resolution_time as timestamp,
    'Issue fully resolved' as description
UNION ALL
SELECT
    'Post-Incident' as event,
    NOW() as timestamp,
    'Review and analysis phase' as description;

SELECT * FROM incident_timeline ORDER BY timestamp;
```

### Step 2: Impact Assessment
```sql
-- Quantify incident impact
SELECT
    'IMPACT ASSESSMENT' as analysis_type,
    incident_duration_minutes as downtime_minutes,
    affected_users_count as users_impacted,
    business_impact_dollars as financial_impact,
    data_loss_mb as data_loss,
    customer_tickets_created as support_tickets
FROM incident_metrics;
```

### Step 3: Data Collection
- [ ] **Logs**: Collect relevant system and application logs
- [ ] **Metrics**: Gather monitoring data from incident period
- [ ] **Communications**: Compile all incident communications
- [ ] **Actions Taken**: Document all troubleshooting and resolution steps
- [ ] **Test Results**: Include any testing performed during incident

## Phase 2: Root Cause Analysis (1 hour)

### Step 4: 5-Why Analysis
Use the 5-Why technique to drill down to root cause:

1. **Why did the incident occur?**
   - *Answer: [Immediate cause]*

2. **Why did that happen?**
   - *Answer: [Contributing factor]*

3. **Why did that happen?**
   - *Answer: [System weakness]*

4. **Why did that happen?**
   - *Answer: [Process gap]*

5. **Why did that happen?**
   - *Answer: [Root cause]*

### Step 5: Contributing Factors Analysis
```sql
-- Analyze contributing factors
CREATE TEMP TABLE contributing_factors AS
SELECT
    'People' as category,
    factor_description,
    impact_level,
    prevention_measure
FROM incident_factors WHERE category = 'People'

UNION ALL

SELECT
    'Process' as category,
    factor_description,
    impact_level,
    prevention_measure
FROM incident_factors WHERE category = 'Process'

UNION ALL

SELECT
    'Technology' as category,
    factor_description,
    impact_level,
    prevention_measure
FROM incident_factors WHERE category = 'Technology'

UNION ALL

SELECT
    'Environment' as category,
    factor_description,
    impact_level,
    prevention_measure
FROM incident_factors WHERE category = 'Environment';

SELECT * FROM contributing_factors ORDER BY impact_level DESC, category;
```

### Step 6: Root Cause Determination
- [ ] **Single Root Cause**: Identify the primary cause
- [ ] **Contributing Causes**: List secondary factors
- [ ] **Prevention Focus**: Determine which causes are preventable
- [ ] **Systemic Issues**: Identify process or architectural problems

## Phase 3: Response Effectiveness Review (45 minutes)

### Step 7: Timeline Analysis
```sql
-- Analyze response effectiveness
SELECT
    phase,
    planned_duration_minutes,
    actual_duration_minutes,
    (actual_duration_minutes - planned_duration_minutes) as variance_minutes,
    CASE
        WHEN actual_duration_minutes > planned_duration_minutes * 1.5 THEN 'SIGNIFICANT_DELAY'
        WHEN actual_duration_minutes > planned_duration_minutes THEN 'MINOR_DELAY'
        WHEN actual_duration_minutes < planned_duration_minutes THEN 'FASTER_THAN_PLANNED'
        ELSE 'ON_TIME'
    END as performance_rating
FROM incident_response_phases
ORDER BY phase_order;
```

### Step 8: Process Adherence Review
- [ ] **Detection**: Was incident detected promptly?
- [ ] **Assessment**: Was severity correctly assessed?
- [ ] **Communication**: Were stakeholders properly informed?
- [ ] **Escalation**: Did escalation happen at appropriate times?
- [ ] **Resolution**: Was resolution approach correct?
- [ ] **Documentation**: Was incident properly documented?

### Step 9: Tool and Runbook Effectiveness
- [ ] **Runbooks Used**: Were appropriate runbooks available and effective?
- [ ] **Tools Available**: Did team have necessary tools and access?
- [ ] **Automation**: Could any manual steps be automated?
- [ ] **Knowledge Gaps**: Were there missing procedures or documentation?

## Phase 4: Improvement Identification (45 minutes)

### Step 10: Corrective Actions
```sql
-- Identify corrective actions
CREATE TEMP TABLE corrective_actions AS
SELECT
    action_category,
    action_description,
    priority,
    owner,
    target_completion_date,
    success_measure
FROM proposed_improvements
WHERE action_type = 'Corrective';

SELECT * FROM corrective_actions ORDER BY priority DESC;
```

### Step 11: Preventive Measures
```sql
-- Identify preventive measures
CREATE TEMP TABLE preventive_measures AS
SELECT
    prevention_category,
    measure_description,
    expected_impact,
    implementation_effort,
    cost_estimate
FROM proposed_improvements
WHERE action_type = 'Preventive';

SELECT * FROM preventive_measures ORDER BY expected_impact DESC;
```

### Step 12: Process Improvements
- [ ] **Detection Improvements**: Better monitoring and alerting
- [ ] **Response Improvements**: Faster escalation and communication
- [ ] **Resolution Improvements**: Better tools and procedures
- [ ] **Prevention Improvements**: Proactive measures and training

## Phase 5: Action Planning and Follow-up (30 minutes)

### Step 13: Action Item Assignment
```sql
-- Create action tracking table
CREATE TABLE incident_followup_actions (
    action_id SERIAL PRIMARY KEY,
    incident_id TEXT,
    action_description TEXT,
    owner TEXT,
    priority TEXT,
    status TEXT DEFAULT 'PENDING',
    target_date DATE,
    completion_date DATE,
    notes TEXT,
    created_at TIMESTAMP DEFAULT NOW()
);

-- Insert identified actions
INSERT INTO incident_followup_actions (incident_id, action_description, owner, priority, target_date)
SELECT
    'INCIDENT_ID',
    action_description,
    owner,
    priority,
    target_completion_date
FROM corrective_actions;
```

### Step 14: Timeline and Accountability
- [ ] **Immediate Actions**: Complete within 1 week
- [ ] **Short-term Actions**: Complete within 1 month
- [ ] **Medium-term Actions**: Complete within 3 months
- [ ] **Long-term Actions**: Complete within 6-12 months
- [ ] **Accountability**: Assign owners and track progress

### Step 15: Success Metrics
Define how to measure the effectiveness of implemented improvements:
- [ ] **MTTR Reduction**: Mean Time To Resolution should decrease
- [ ] **MTTD Improvement**: Mean Time To Detection should improve
- [ ] **Recurrence Prevention**: Similar incidents should not recur
- [ ] **Process Adherence**: Response processes should be followed consistently

## Post-Incident Review Template

### Incident Summary
- **Incident ID**: [Unique identifier]
- **Date/Time**: [When incident occurred]
- **Duration**: [How long it lasted]
- **Severity**: [SEV 1/2/3/4]
- **Affected Systems**: [Which systems impacted]
- **Business Impact**: [Description of business effects]

### What Happened
- **Trigger**: [What initiated the incident]
- **Symptoms**: [What was observed]
- **Scope**: [How widespread was the impact]
- **Detection**: [How was it discovered]

### Root Cause
- **Primary Cause**: [The main reason]
- **Contributing Factors**: [Secondary causes]
- **Prevention Gaps**: [What could have prevented it]

### Response Analysis
- **Strengths**: [What went well]
- **Weaknesses**: [What didn't go well]
- **Timeline**: [Key timestamps and durations]
- **Communication**: [How information flowed]

### Lessons Learned
- **Technical Lessons**: [System and technical insights]
- **Process Lessons**: [Response and operational insights]
- **Team Lessons**: [Collaboration and coordination insights]

### Action Items
| Action | Owner | Priority | Target Date | Status |
|--------|-------|----------|-------------|--------|
| [Action 1] | [Owner] | [Priority] | [Date] | [Status] |
| [Action 2] | [Owner] | [Priority] | [Date] | [Status] |

### Follow-up Review
- **Review Date**: [When to check progress]
- **Success Criteria**: [How to measure improvement]
- **Escalation**: [What to do if actions not completed]

## Common Post-Incident Anti-patterns

### ❌ What Not to Do
- **Blame Assignment**: Focus on systems and processes, not people
- **Superficial Analysis**: "Server crashed" is not a root cause
- **No Follow-through**: Actions identified but never implemented
- **Overly Broad Actions**: Trying to fix everything at once
- **Ignoring Data**: Making decisions without evidence

### ✅ Best Practices
- **Focus on Learning**: Every incident is a learning opportunity
- **Data-Driven**: Base conclusions on evidence, not opinions
- **Action-Oriented**: Identify specific, measurable improvements
- **Collaborative**: Include all stakeholders in analysis
- **Timely**: Complete review while details are fresh

## Related Runbooks

- [Incident Checklist](incident-checklist.md) - Incident response process
- [Emergency Procedures](emergency-procedures.md) - Crisis response
- [TVIEW Health Check](../01-health-monitoring/tview-health-check.md) - Ongoing monitoring
- [Performance Monitoring](../01-health-monitoring/performance-monitoring.md) - Proactive monitoring

## Metrics to Track

### Incident Review Metrics
- **Review Completion Rate**: Percentage of incidents with completed reviews
- **Action Implementation Rate**: Percentage of identified actions implemented
- **Recurrence Rate**: How often similar incidents happen
- **Time to Review**: How quickly reviews are completed

### Process Improvement Metrics
- **MTTR Trends**: Mean Time To Resolution over time
- **MTTD Trends**: Mean Time To Detection over time
- **Severity Distribution**: How incident severity changes
- **Process Adherence**: How well response processes are followed

## Templates and Checklists

### Post-Incident Review Checklist
- [ ] Timeline reconstructed and verified
- [ ] Root cause identified and agreed upon
- [ ] Contributing factors documented
- [ ] Impact fully assessed
- [ ] Response effectiveness evaluated
- [ ] Corrective actions identified
- [ ] Preventive measures planned
- [ ] Action items assigned with owners and dates
- [ ] Success metrics defined
- [ ] Follow-up review scheduled
- [ ] Documentation completed and shared

### Action Item Template
```
Action: [Clear, specific description]
Owner: [Person responsible]
Priority: [High/Medium/Low]
Target Date: [Specific date]
Success Criteria: [How to measure completion]
Dependencies: [What needs to happen first]
Resources Needed: [Tools, budget, or help required]
```

## Continuous Improvement

### Regular Review Cadence
- **Weekly**: Review any incidents from past week
- **Monthly**: Analyze incident trends and patterns
- **Quarterly**: Review process effectiveness and make systemic improvements
- **Annually**: Major process reviews and training updates

### Knowledge Sharing
- **Incident Database**: Maintain searchable incident history
- **Lessons Learned Sessions**: Regular team discussions
- **Training Updates**: Incorporate lessons into training programs
- **Process Documentation**: Keep runbooks current with lessons learned

Remember: The goal of post-incident reviews is not to assign blame, but to improve systems, processes, and team capabilities to prevent future incidents and respond more effectively when they do occur.</content>
<parameter name="filePath">docs/operations/runbooks/04-incident-response/post-incident-review.md