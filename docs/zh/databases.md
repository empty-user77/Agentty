---
title: 数据库
description: 在 Agentty 中查看项目已经配置好的数据库，让智能体查询它们，而写操作必须经你批准。
---

Agentty 会找出项目配置的数据库，在终端旁的页面中展示，并让智能体可以查询。读取立即执行；任何可能改变数据的操作都要等你按下 **Execute**。

## 如何找到连接

Agentty 读取项目**自身的**配置 —— 从不向你的项目写入任何东西：

| 来源 | 读取内容 |
|---|---|
| `.env` 文件 | 连接 URL 以及 host / user / password / database 变量 |
| Spring | `application.properties` / `application-<profile>.yml` |
| Prisma | `datasource` 块 |
| Compose | 使用 MySQL、MariaDB、PostgreSQL、MongoDB 或 Oracle 镜像的服务 |

`${VAR}`、`${VAR:默认值}`、`env("VAR")` 和 `$VAR` 这类引用会从项目的 `.env` 文件解析。如果某个值最终仍是引用 —— 来自密钥管理器，或在别处设置的环境变量 —— 该字段就显示为缺失，你只需填一次。

支持的引擎有 MySQL、MariaDB、PostgreSQL、MongoDB 和 Oracle。Oracle 还需要额外安装 Oracle Instant Client。

## 补齐缺失的值

- 数据库页面上的**输入密码**会把密码存进操作系统的凭据存储，而不是文件。连接 id 由项目和地址推导而来，所以即使配置文件变了，密码也跟着这个连接走。
- **添加**用于手工定义连接（引擎、主机、端口、数据库、用户、密码），保存在 `~/.agentty/db-connections.json` 中，仅所有者可读，**且不含密码**。

## 智能体能跑什么，不能跑什么

这部分值得了解，因为它正是把数据库交给智能体仍然合理的原因。

每一条语句在执行前都会被分类：

- **读取** —— 以 `SELECT`、`SHOW`、`DESCRIBE`、`EXPLAIN`、`WITH … SELECT`、`VALUES` 或 `TABLE` 开头、且不含写关键字的单条语句 —— 立即执行并显示结果行。
- **其余一律进入待批准队列。** 页面会显示是谁发起的、哪个连接、什么类型的语句，以及确切原文。只有当你按下 **Execute** 才会执行，智能体的命令最多等待 15 分钟。

分类器刻意保持多疑：一次多条语句、可执行注释、美元引用、未闭合的引号，以及对已知纯计算内置函数（`COUNT`、`COALESCE`、`DATE_FORMAT` 等）之外的任何调用，都需要批准。存储函数和扩展即便在只读事务中也可能写入，因此绝不自动放行。

对 MongoDB，`find`、`count`、`distinct`、`listCollections` 以及仅由已知读取阶段组成的聚合算作读取；`$out`、`$merge` 或未知阶段需要批准。

### 第二道防线

读取本身也在**一个会被回滚的只读事务中**执行（`START TRANSACTION READ ONLY`、`BEGIN READ ONLY`、`SET TRANSACTION READ ONLY`）。万一分类器判断失误，数据库自己也会拒绝写入。

结果有上限 —— 页面 200 行、智能体 500 行，单元格在 4000 字符处截断 —— 所以一条随手的 `SELECT *` 不会淹没窗格。

## 连接与 TLS

对 `localhost`、`127.0.0.1`、`::1`，或配置明确禁用时（`sslmode=disable`、`useSSL=false`），不使用 TLS。其余情况下 TLS 是必需的，并以系统根证书加上内置的 Amazon RDS 证书包进行校验，因此 RDS 端点无需额外配置即可使用 —— 主机填实例端点即可。

密码不会出现在任何响应、页面或日志中。连接错误只会提到主机、用户和数据库。

## 使用方式

数据库标记和页面跟随活动窗格的工作树，与 Docker 面板一致。无论是在页面的查询框中执行，还是智能体通过 `agentty db` 执行，都走同一套分类与批准流程。
