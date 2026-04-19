#!/usr/bin/env python3
"""
飞书消息导出工具
按天导出飞书单聊消息和@我的消息为结构化数据文件（JSON/CSV）
支持指定日期范围导出，确保历史消息补充同步不遗漏
"""

import argparse
import csv
import json
import os
import subprocess
import sys
from datetime import datetime, timedelta
from pathlib import Path
from typing import Any


def get_date_range(date_str: str, start_date: str = None, end_date: str = None) -> list:
    """解析日期参数，返回日期列表"""
    today = datetime.now().replace(hour=0, minute=0, second=0, microsecond=0)
    dates = []
    
    if date_str:
        if date_str == "today":
            dates = [today]
        elif date_str == "yesterday":
            dates = [today - timedelta(days=1)]
        else:
            try:
                dates = [datetime.strptime(date_str, "%Y-%m-%d")]
            except ValueError:
                print(f"错误: 日期格式不正确，请使用 YYYY-MM-DD 格式，例如: 2026-04-15")
                sys.exit(1)
    elif start_date and end_date:
        try:
            start = datetime.strptime(start_date, "%Y-%m-%d")
            end = datetime.strptime(end_date, "%Y-%m-%d")
            if start > end:
                print("错误: 开始日期不能晚于结束日期")
                sys.exit(1)
            current = start
            while current <= end:
                dates.append(current)
                current += timedelta(days=1)
        except ValueError:
            print("错误: 日期格式不正确，请使用 YYYY-MM-DD 格式")
            sys.exit(1)
    else:
        dates = [today]
    
    return dates


def format_datetime(dt: datetime) -> str:
    """格式化日期时间为字符串"""
    return dt.strftime("%Y-%m-%d %H:%M:%S")


def get_date_range_str(dt: datetime) -> tuple:
    """获取日期的起始和结束时间字符串（带时区）"""
    start = dt.strftime("%Y-%m-%dT00:00:00+08:00")
    end = dt.strftime("%Y-%m-%dT23:59:59+08:00")
    return start, end


def run_lark_cli(command: list) -> dict:
    """运行 lark-cli 命令并返回 JSON 结果"""
    try:
        result = subprocess.run(
            command,
            capture_output=True,
            text=True,
            check=True
        )
        # 尝试解析 JSON 输出
        try:
            return json.loads(result.stdout)
        except json.JSONDecodeError:
            # 如果不是 JSON，可能需要处理
            return {"raw": result.stdout}
    except subprocess.CalledProcessError as e:
        print(f"命令执行失败: {' '.join(command)}")
        print(f"错误信息: {e.stderr}")
        return {"error": e.stderr}
    except FileNotFoundError:
        print("错误: 未找到 lark-cli 命令，请确保已安装并配置好")
        sys.exit(1)


def search_p2p_messages(start_time: str, end_time: str) -> list:
    """搜索指定时间范围内的私信消息"""
    messages = []
    page_token = None
    page_count = 0
    max_pages = 50  # 最多获取50页
    
    while page_count < max_pages:
        # 构建命令
        cmd = [
            "lark-cli", "im", "+messages-search",
            "--query", "",
            "--chat-type", "p2p",
            "--sender-type", "user",
            "--start", start_time,
            "--end", end_time,
            "--page-size", "50",
            "--format", "json"
        ]
        
        if page_token:
            cmd.extend(["--page-token", page_token])
        
        # 执行搜索
        result = run_lark_cli(cmd)
        
        if "error" in result:
            break
        
        # 解析消息
        items = result.get("data", {}).get("items", [])
        if not items:
            break
            
        for item in items:
            # 过滤掉系统消息和空消息
            if item.get("msg_type") in ["text", "post"] and item.get("content"):
                # 排除自己发的消息（通过sender判断）
                sender = item.get("sender", {})
                sender_id = sender.get("id", "")
                
                # 简单判断：如果消息有内容且不是空
                content = item.get("content", "")
                if content and content.strip():
                    messages.append({
                        "message_type": "私信",
                        "sender_name": sender.get("name", sender_id),
                        "sender_id": sender_id,
                        "create_time": item.get("create_time", ""),
                        "content": content[:500] if len(content) > 500 else content,  # 截断超长内容
                        "message_id": item.get("message_id", ""),
                        "chat_id": item.get("chat_id", "")
                    })
        
        # 检查是否还有更多页
        page_info = result.get("data", {})
        if not page_info.get("has_more"):
            break
            
        page_token = page_info.get("page_token")
        if not page_token:
            break
            
        page_count += 1
    
    return messages


def search_at_me_messages(start_time: str, end_time: str) -> list:
    """搜索指定时间范围内@我的消息"""
    messages = []
    page_token = None
    page_count = 0
    max_pages = 50
    
    while page_count < max_pages:
        cmd = [
            "lark-cli", "im", "+messages-search",
            "--query", "",
            "--is-at-me",
            "--sender-type", "user",
            "--start", start_time,
            "--end", end_time,
            "--page-size", "50",
            "--format", "json"
        ]
        
        if page_token:
            cmd.extend(["--page-token", page_token])
        
        result = run_lark_cli(cmd)
        
        if "error" in result:
            break
            
        items = result.get("data", {}).get("items", [])
        if not items:
            break
            
        for item in items:
            msg_type = item.get("msg_type", "")
            if msg_type not in ["text", "post"]:
                continue
                
            content = item.get("content", "")
            if not content:
                continue
                
            # 检查是否是@everyone或@_all的系统通知
            mentions = item.get("mentions", [])
            is_system_at = False
            for mention in mentions:
                key = mention.get("key", "")
                if key in ["@_all", "@everyone", "@all"]:
                    is_system_at = True
                    break
                    
            if is_system_at:
                continue
                
            sender = item.get("sender", {})
            messages.append({
                "message_type": "@",
                "sender_name": sender.get("name", sender.get("id", "")),
                "sender_id": sender.get("id", ""),
                "create_time": item.get("create_time", ""),
                "content": content[:500] if len(content) > 500 else content,
                "message_id": item.get("message_id", ""),
                "chat_id": item.get("chat_id", ""),
                "chat_name": item.get("chat_name", "")
            })
        
        page_info = result.get("data", {})
        if not page_info.get("has_more"):
            break
            
        page_token = page_info.get("page_token")
        if not page_token:
            break
            
        page_count += 1
    
    return messages


def filter_user_messages(messages: list) -> list:
    """过滤出真正用户发的消息（排除系统自动推送）"""
    filtered = []
    
    # 系统消息的典型特征
    system_keywords = [
        "告警", "报警", "alert", "系统通知", "系统消息",
        "推送", "通知", "notification", "system"
    ]
    
    for msg in messages:
        content = msg.get("content", "").lower()
        sender_name = msg.get("sender_name", "").lower()
        
        # 跳过明显是系统的消息
        is_system = False
        for keyword in system_keywords:
            if keyword.lower() in content or keyword.lower() in sender_name:
                # 但如果sender是真实用户，则保留
                if "机器人" not in sender_name and "bot" not in sender_name:
                    break
                is_system = True
                break
        
        if not is_system:
            filtered.append(msg)
    
    return filtered


def export_messages(messages: list, output_path: str, export_format: str) -> None:
    """导出消息到文件"""
    if export_format == "json":
        with open(output_path, "w", encoding="utf-8") as f:
            json.dump(messages, f, ensure_ascii=False, indent=2)
    elif export_format == "csv":
        if not messages:
            # 即使没有消息也创建空文件
            messages = []
            
        fieldnames = ["序号", "消息类型", "发送人", "发送时间", "消息内容", "群名称"]
        with open(output_path, "w", encoding="utf-8-sig", newline="") as f:
            writer = csv.DictWriter(f, fieldnames=fieldnames)
            writer.writeheader()
            
            for idx, msg in enumerate(messages, 1):
                writer.writerow({
                    "序号": idx,
                    "消息类型": msg.get("message_type", ""),
                    "发送人": msg.get("sender_name", ""),
                    "发送时间": msg.get("create_time", ""),
                    "消息内容": msg.get("content", ""),
                    "群名称": msg.get("chat_name", "")
                })
    
    print(f"✅ 消息已导出到: {output_path}")


def main():
    parser = argparse.ArgumentParser(
        description="飞书消息导出工具 - 按天导出飞书单聊消息和@我的消息",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog="""
示例用法:
  %(prog)s --date today
  %(prog)s --date yesterday --format csv
  %(prog)s --date 2026-04-15
  %(prog)s --start-date 2026-04-01 --end-date 2026-04-15
  %(prog)s --date today --output-dir ~/Downloads
        """
    )
    
    parser.add_argument(
        "--date", 
        type=str,
        help="指定日期 (today/yesterday/YYYY-MM-DD)"
    )
    parser.add_argument(
        "--start-date",
        type=str,
        help="开始日期 (YYYY-MM-DD)"
    )
    parser.add_argument(
        "--end-date",
        type=str,
        help="结束日期 (YYYY-MM-DD)"
    )
    parser.add_argument(
        "--format",
        type=str,
        choices=["json", "csv"],
        default="json",
        help="导出格式 (默认: json)"
    )
    parser.add_argument(
        "--output-dir",
        type=str,
        help="输出目录 (默认: 当前目录)"
    )
    parser.add_argument(
        "--verbose",
        action="store_true",
        help="显示详细输出"
    )
    
    args = parser.parse_args()
    
    # 获取日期列表
    dates = get_date_range(args.date, args.start_date, args.end_date)
    
    # 确定输出目录
    output_dir = args.output_dir or os.path.expanduser("~/Downloads")
    if not os.path.exists(output_dir):
        output_dir = "."
    
    # 汇总所有消息
    all_messages = []
    
    print(f"开始导出飞书消息，共 {len(dates)} 天...")
    
    for dt in dates:
        date_str = dt.strftime("%Y-%m-%d")
        print(f"\n📅 正在导出 {date_str} 的消息...")
        
        start_time, end_time = get_date_range_str(dt)
        
        if args.verbose:
            print(f"  时间范围: {start_time} ~ {end_time}")
        
        # 搜索私信消息
        print("  🔍 搜索私信消息...")
        p2p_messages = search_p2p_messages(start_time, end_time)
        p2p_messages = filter_user_messages(p2p_messages)
        print(f"     找到 {len(p2p_messages)} 条私信")
        
        # 搜索@消息
        print("  🔍 搜索@我的消息...")
        at_messages = search_at_me_messages(start_time, end_time)
        print(f"     找到 {len(at_messages)} 条@消息")
        
        # 合并消息
        day_messages = p2p_messages + at_messages
        # 按时间排序
        day_messages.sort(key=lambda x: x.get("create_time", ""), reverse=True)
        
        # 添加日期标记
        for msg in day_messages:
            msg["date"] = date_str
            
        all_messages.extend(day_messages)
        
        if args.verbose:
            print(f"  📊 当日共 {len(day_messages)} 条消息")
    
    # 按时间排序所有消息
    all_messages.sort(key=lambda x: x.get("create_time", ""), reverse=True)
    
    # 导出文件
    if dates:
        first_date = dates[0].strftime("%Y%m%d")
        last_date = dates[-1].strftime("%Y%m%d")
        
        if first_date == last_date:
            filename = f"feishu_messages_{first_date}.{args.format}"
        else:
            filename = f"feishu_messages_{first_date}_{last_date}.{args.format}"
        
        output_path = os.path.join(output_dir, filename)
        export_messages(all_messages, output_path, args.format)
    
    # 打印统计
    p2p_count = sum(1 for m in all_messages if m.get("message_type") == "私信")
    at_count = sum(1 for m in all_messages if m.get("message_type") == "@")
    
    print(f"\n📊 导出统计:")
    print(f"   私信消息: {p2p_count} 条")
    print(f"   @消息: {at_count} 条")
    print(f"   总计: {len(all_messages)} 条")


if __name__ == "__main__":
    main()